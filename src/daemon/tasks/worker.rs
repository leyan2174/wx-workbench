use super::{now, process, Service, Work};
use crate::{
    attachment::local_files::HostOutputGuard,
    service::{config_pin::ConfigPin, plan, protocol::Kind},
};
use anyhow::{ensure, Result};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
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
    total: Arc<AtomicU64>,
) -> Result<()> {
    let mut chunk = [0; 2048];
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let bytes = total.fetch_add(count as u64, Ordering::Relaxed) + count as u64;
        if suppress {
            use zeroize::Zeroize;
            chunk[..count].zeroize();
            ensure!(bytes <= 64 * 1024 * 1024, "Worker output limit exceeded");
            continue;
        }
        ensure!(bytes <= 64 * 1024 * 1024, "Worker output limit exceeded");
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
    Ok(())
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
    // One generous total budget across all steps; large offline exports remain supported.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(24 * 60 * 60);
    let result: Result<Option<i32>> = async {
        let locked = ConfigPin::new(&state.runtime)?;
        ensure!(
            locked.fingerprint()? == work.fingerprint,
            "Queued task configuration changed"
        );
        pin = Some(locked);
        let steps = plan::plan(
            &work.request,
            &work.settings,
            &state.runtime.config_path,
            &task.output_dir,
        )?;
        for source in [
            Some(&state.runtime.config_path),
            Some(&state.runtime.config.keys_file),
            state.runtime.config.key_store.as_ref(),
            Some(&state.runtime.config.db_dir),
            Some(&state.runtime.config.decrypted_dir),
            Some(&state.runtime.directory),
            work.settings.image_cache_dir.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            crate::infrastructure::publication::separate(source, &task.output_dir)?;
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
        if matches!(task.kind, Kind::ExportAll | Kind::ExportHistory) {
            super::artifacts::prepare(&state.runtime, &task.id)?;
            if task.kind == Kind::ExportHistory {
                super::history_artifacts::start(&state.runtime, &task)?;
            }
        }
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
            let (mut child, job, _registration) = process::spawn(&state.runtime, step, &state.keys).await?;
            let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
                process::reap(child, job).await?;
                anyhow::bail!("Task output handles unavailable");
            };
            let total = Arc::new(AtomicU64::new(0));
            let mut out = tokio::spawn(drain(
                stdout,
                state.clone(),
                work.id.clone(),
                "stdout",
                suppress,
                total.clone(),
            ));
            let mut err = tokio::spawn(drain(
                stderr,
                state.clone(),
                work.id.clone(),
                "stderr",
                suppress,
                total,
            ));
            let (mut out_done, mut err_done) = (false, false);
            let exit: Result<_> = async {
                loop {
                    tokio::select! { biased;
                        _ = signalled(&mut work.cancel) => return Ok(None),
                        _ = signalled(shutdown) => return Ok(None),
                        _ = tokio::time::sleep_until(deadline) => anyhow::bail!("Worker task deadline expired"),
                        result = &mut out, if !out_done => { out_done = true; result??; },
                        result = &mut err, if !err_done => { err_done = true; result??; },
                        result = child.wait() => return Ok(Some(result)),
                    }
                }
            }.await;
            let cleanup = process::reap(child, job).await;
            let mut pipes = Ok(());
            for (reader, done) in [(&mut out, out_done), (&mut err, err_done)] {
                if done { continue; }
                match tokio::time::timeout(Duration::from_secs(2), &mut *reader).await {
                    Ok(Ok(Ok(()))) => {},
                    Ok(_) => pipes = Err(anyhow::anyhow!("Worker output reader failed")),
                    Err(_) => {
                        reader.abort();
                        let _ = reader.await;
                        pipes = Err(anyhow::anyhow!("Worker output cleanup timed out"));
                    }
                }
            }
            cleanup?;
            pipes?;
            let exit = exit?;
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
    let identity_changed = pin
        .as_ref()
        .is_some_and(|pin| pin.verify(&state.runtime).is_err());
    if identity_changed {
        state.log(&work.id, "system", "配置身份复核失败，后台停止接受任务");
        state.request_shutdown();
    }
    let export_result = if matches!(task.kind, Kind::ExportAll | Kind::ExportHistory)
        && !identity_changed
    {
        let runtime = state.runtime.clone();
        let task = task.clone();
        let mut control =
            super::artifacts::FinalizeControl::new(work.cancel.clone(), shutdown.clone(), deadline);
        Some(
            tokio::task::spawn_blocking(move || {
                super::artifacts::finalize_task(&runtime, &task, &mut control)
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result),
        )
    } else {
        None
    };
    let cancelled = *work.cancel.borrow() || *shutdown.borrow();
    state.update(&work.id, |task| {
        task.finished_at = Some(now());
        match result {
            _ if identity_changed => {
                task.status = "failed".into();
                task.error = Some("配置身份复核失败，后台已停止".into());
            }
            Ok(_) if cancelled => task.status = "cancelled".into(),
            Ok(None) => task.status = "cancelled".into(),
            Ok(Some(code)) => {
                let outcome = crate::ipc::outcome::BusinessOutcome::from_worker_exit(code);
                task.exit_code = Some(code);
                task.status = if outcome == crate::ipc::outcome::BusinessOutcome::Success {
                    "succeeded"
                } else {
                    "failed"
                }
                .into();
                if outcome != crate::ipc::outcome::BusinessOutcome::Success {
                    task.error = Some(outcome.public_message().into());
                }
            }
            Err(error) => {
                task.status = "failed".into();
                task.error = Some(error.downcast_ref::<crate::key_store::Error>().map_or_else(
                    || "任务预检、启动或执行失败，未公开内部诊断".into(),
                    ToString::to_string,
                ));
            }
        }
        if let Some(export_result) = export_result {
            match export_result {
                Ok(report) => {
                    attach_export_report(task, report);
                }
                Err(_) => {
                    if task.status == "succeeded" {
                        task.status = "failed".into();
                    }
                    task.error = Some("result_unavailable".into());
                }
            }
        }
    });
}

pub(super) fn attach_export_report(
    task: &mut crate::service::protocol::Task,
    report: crate::service::task_artifacts::TaskResult,
) {
    if !report.validate(task.kind) {
        task.status = "failed".into();
        task.error = Some("result_unavailable".into());
        return;
    }
    if !report.artifacts_complete() && task.status == "succeeded" {
        task.status = "failed".into();
        task.error = Some("result_unavailable".into());
    }
    task.result = Some(report);
}
