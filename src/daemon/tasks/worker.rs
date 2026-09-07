use super::{now, process, store::Redactor, Service, Work};
use crate::{
    attachment::local_files::HostOutputGuard,
    service::{config_pin::ConfigPin, plan, protocol::Kind},
};
use anyhow::{ensure, Result};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{mpsc, watch},
};

async fn signalled(receiver: &mut watch::Receiver<bool>) {
    if !*receiver.borrow() {
        let _ = receiver.changed().await;
    }
}

async fn drain<R: AsyncRead + Unpin>(
    mut pipe: R,
    state: Arc<Service>,
    id: String,
    stream: &'static str,
    suppress: bool,
) {
    let mut chunk = [0; 2048];
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let count = match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if suppress {
            use zeroize::Zeroize;
            chunk[..count].zeroize();
            continue;
        }
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if oversized {
                    state.log(&id, stream, "[oversized line omitted]");
                } else {
                    state.log(&id, stream, &String::from_utf8_lossy(&line));
                }
                line.clear();
                oversized = false;
            } else if line.len() < 8192 {
                line.push(*byte);
            } else {
                oversized = true;
            }
        }
    }
    if oversized {
        state.log(&id, stream, "[oversized line omitted]");
    } else if !line.is_empty() {
        state.log(&id, stream, &String::from_utf8_lossy(&line));
    }
}

pub async fn run(state: Arc<Service>, mut queue: mpsc::Receiver<Work>) {
    let mut shutdown = state.shutdown.subscribe();
    loop {
        let work = tokio::select! { biased;
            _ = signalled(&mut shutdown) => break,
            work = queue.recv() => match work { Some(work) => work, None => break },
        };
        execute(state.clone(), work, &mut shutdown).await;
    }
    queue.close();
    while let Some(work) = queue.recv().await {
        state.update(&work.id, |task| {
            task.status = "cancelled".into();
            task.finished_at = Some(now());
        });
    }
}

async fn execute(state: Arc<Service>, mut work: Work, shutdown: &mut watch::Receiver<bool>) {
    if *work.cancel.borrow() || *shutdown.borrow() {
        state.update(&work.id, |task| {
            task.status = "cancelled".into();
            task.finished_at = Some(now());
        });
        return;
    }
    let Some(task) = state.update(&work.id, |task| {
        task.status = "running".into();
        task.started_at = Some(now());
    }) else {
        return;
    };
    if task.terminal() {
        return;
    }
    let mut pin = None;
    let mut released = false;
    let result: Result<Option<i32>> = async {
        let locked = ConfigPin::new(&state.runtime)?;
        ensure!(
            locked.fingerprint()? == work.fingerprint,
            "Queued task configuration changed"
        );
        pin = Some(locked);
        *state.redactor.lock().unwrap() = Redactor::new(&state.runtime)?;
        let steps = plan::plan(
            &work.request,
            &work.settings,
            &state.runtime.config_path,
            &task.output_dir,
        )?;
        for source in [
            Some(&state.runtime.config_path),
            Some(&state.runtime.config.keys_file),
            Some(&state.runtime.config.db_dir),
            Some(&state.runtime.config.decrypted_dir),
            Some(&state.runtime.directory),
            work.settings.enterprise_snapshot.as_ref(),
            work.settings.enterprise_data_dir.as_ref(),
            work.settings.enterprise_input.as_ref(),
            work.settings.enterprise_key_file.as_ref(),
            work.settings.enterprise_keys_file.as_ref(),
            work.settings.image_cache_dir.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            crate::toolkit::separate(source, &task.output_dir)?;
        }
        let parent = task
            .output_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing task output parent"))?;
        tokio::fs::create_dir_all(parent).await?;
        let parent_guard = HostOutputGuard::new(parent)?;
        parent_guard.verify()?;
        // A task never adopts a pre-existing output root, including one created after submission.
        tokio::fs::create_dir(&task.output_dir).await?;
        let output_guard = HostOutputGuard::new(&task.output_dir)?;
        for (index, step) in steps.iter().enumerate() {
            if *work.cancel.borrow() || *shutdown.borrow() {
                return Ok(None);
            }
            parent_guard.verify()?;
            output_guard.verify()?;
            state.log(
                &work.id,
                "system",
                &format!("开始步骤 {}/{}", index + 1, steps.len()),
            );
            let suppress = matches!(work.request.kind, Kind::ImageKey | Kind::WechatKeys);
            if work.request.kind == Kind::ImageKey {
                pin.as_mut()
                    .unwrap()
                    .release_for_image_key(&state.runtime)?;
                released = true;
            }
            let (mut child, job) = process::spawn(&state.runtime, step).await?;
            let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
                process::reap(child, job).await;
                anyhow::bail!("Task output handles unavailable");
            };
            let mut out = tokio::spawn(drain(
                stdout,
                state.clone(),
                work.id.clone(),
                "stdout",
                suppress,
            ));
            let mut err = tokio::spawn(drain(
                stderr,
                state.clone(),
                work.id.clone(),
                "stderr",
                suppress,
            ));
            let exit = tokio::select! { biased;
                _ = signalled(&mut work.cancel) => None,
                _ = signalled(shutdown) => None,
                result = child.wait() => Some(result),
            };
            process::reap(child, job).await;
            for reader in [&mut out, &mut err] {
                if tokio::time::timeout(Duration::from_secs(2), &mut *reader)
                    .await
                    .is_err()
                {
                    reader.abort();
                    let _ = reader.await;
                }
            }
            let Some(exit) = exit else { return Ok(None) };
            let status = exit?;
            if !status.success() {
                return Ok(Some(status.code().unwrap_or(-1)));
            }
        }
        output_guard.verify()?;
        parent_guard.verify()?;
        Ok(Some(0))
    }
    .await;
    let config_error = if released {
        pin.as_mut().unwrap().repin(&state.runtime).is_err()
    } else {
        false
    };
    if matches!(work.request.kind, Kind::WechatKeys | Kind::ImageKey) {
        match Redactor::new(&state.runtime) {
            Ok(redactor) => *state.redactor.lock().unwrap() = redactor,
            Err(_) => state.request_shutdown(),
        }
        state.query.invalidate().await;
    }
    if config_error {
        state.log(&work.id, "system", "配置身份复核失败，后台停止接受任务");
        state.request_shutdown();
    }
    let cancelled = *work.cancel.borrow() || *shutdown.borrow();
    if !cancelled
        && matches!(result, Ok(Some(0)))
        && matches!(work.request.kind, Kind::WxworkDecrypt | Kind::WxworkRun)
    {
        if let Some(data) = &work.settings.enterprise_data_dir {
            if let Ok(account) = crate::toolkit::enterprise_batch::Account::open(data) {
                let snapshot = task
                    .output_dir
                    .join("enterprise-snapshot")
                    .join(account.account_id)
                    .join("decrypted");
                if snapshot.is_dir() {
                    state.records.lock().unwrap().enterprise_snapshot = Some(snapshot);
                }
            }
        }
    }
    state.update(&work.id, |task| {
        task.finished_at = Some(now());
        match result {
            _ if config_error => {
                task.status = "failed".into();
                task.error = Some("配置身份复核失败，后台已停止".into());
            }
            _ if cancelled => task.status = "cancelled".into(),
            Ok(None) => task.status = "cancelled".into(),
            Ok(Some(code)) => {
                task.exit_code = Some(code);
                task.status = if code == 0 { "succeeded" } else { "failed" }.into();
                if code != 0 {
                    task.error = Some("任务进程未成功完成，请查看脱敏日志".into());
                }
            }
            Err(_) => {
                task.status = "failed".into();
                task.error = Some("任务预检、启动或执行失败，未公开内部诊断".into());
            }
        }
    });
}
