//! 独立图片密钥命令；只更新固定账号的加密存储，不改配置或输出密钥材料。
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

pub use crate::service::operation_requests::image_keys::Args;

pub(super) fn publication_material(
    runtime: &RuntimeContext,
) -> Result<crate::application::image_publication::StoredImageKeys> {
    let material = crate::service::worker_keys::image_material(runtime)?;
    let revision = crate::service::worker_keys::expected_revision(runtime)?;
    let material = match material {
        Some(material) => crate::application::image_publication::StoredImageKeys {
            aes: Some(material.aes),
            xor: material.xor,
        },
        None if revision != 0 => crate::application::image_publication::StoredImageKeys {
            aes: None,
            xor: 0x88,
        },
        None if runtime.config.key_store.is_none() => {
            return Err(crate::key_store::Error::LegacyMigrationRequired.into())
        }
        None => return Err(crate::key_store::Error::Missing.into()),
    };
    crate::service::worker_keys::verify_image_revision(runtime)?;
    Ok(material)
}

pub fn cmd(args: Args) -> Result<()> {
    ensure!(
        args.offline != args.authorize_memory_scan,
        "请选择 --offline 或 --authorize-memory-scan，不能同时使用"
    );
    let runtime = RuntimeContext::load()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&extract_for(&runtime, args)?)?
    );
    Ok(())
}

fn read_config(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "配置超过 1 MiB 限制");
    Ok(bytes)
}

pub(super) fn extract_for(runtime: &RuntimeContext, args: Args) -> Result<Value> {
    extract_cancellable(runtime, args, &AtomicBool::new(false), false)
}

fn extract_cancellable(
    runtime: &RuntimeContext,
    args: Args,
    cancelled: &AtomicBool,
    reuse_existing: bool,
) -> Result<Value> {
    ensure!(!cancelled.load(Ordering::SeqCst), "图片取钥已取消");
    ensure!(
        args.offline != args.authorize_memory_scan,
        "请选择离线推导或明确授权内存扫描"
    );
    if let Some(expected) = std::env::var_os("WX_CLI_EXPECTED_RUNTIME") {
        ensure!(
            expected.to_str() == Some(runtime.id.as_str()),
            "当前账号与父进程固定账号不一致，未开始扫描"
        );
    }
    ensure!(
        (1..=3600).contains(&args.timeout) && (1..=32768).contains(&args.max_mib),
        "扫描预算超出允许范围"
    );
    let sample = super::image_key_sample::prepare(runtime, &args.sample)?;
    let parent = runtime.config_path.parent().context("配置缺少父目录")?;
    let guard = HostOutputGuard::new(parent)?;
    guard.verify_replaceable_file(&runtime.config_path)?;
    let original = read_config(&runtime.config_path)?;
    let existing = if reuse_existing {
        crate::service::worker_keys::image_material(runtime)?
    } else {
        None
    };
    if !args.no_save {
        crate::service::worker_keys::expected_revision(runtime)?;
    }
    let current = RuntimeContext::load()?;
    ensure!(
        runtime.same_account(&current)?,
        "选中账号配置发生变化，未开始扫描"
    );
    if let Some(existing) = existing {
        let key = Zeroizing::new(existing.aes);
        let valid = crate::attachment::image_key::windows::validate_existing_for_db_dir(
            &runtime.config.db_dir,
            &key,
            Duration::from_secs(args.timeout),
            args.max_mib * 1024 * 1024,
        )?;
        ensure!(!cancelled.load(Ordering::SeqCst), "图片取钥已取消");
        if valid {
            crate::service::worker_keys::verify_image_revision(runtime)?;
            guard.verify_replaceable_file(&runtime.config_path)?;
            ensure!(
                read_config(&runtime.config_path)?.as_slice() == original.as_slice(),
                "验证期间账号配置发生变化"
            );
            let sample_report = if let Some(sample) = sample {
                let mut material = crate::attachment::image_key::ImageKeyMaterial {
                    aes_key: *key,
                    xor_key: existing.xor,
                };
                let staged = sample.stage(&material);
                material.aes_key.zeroize();
                let staged = staged?;
                ensure!(
                    !cancelled.load(Ordering::SeqCst),
                    "图片取钥已取消，未发布样本"
                );
                Some(staged.publish()?)
            } else {
                None
            };
            return Ok(json!({"engine":"rust", "account_id":runtime.id,
                    "existing_key_valid":true, "config_updated":false, "key_store_updated":false, "keys_redacted":true, "sample":sample_report}));
        }
    }
    let mut material = if args.offline {
        crate::attachment::image_key::offline::extract_for_db_dir(
            &runtime.config.db_dir,
            Duration::from_secs(args.timeout),
            args.max_mib * 1024 * 1024,
            cancelled,
        )?
    } else {
        crate::attachment::image_key::windows::extract_for_db_dir(
            &runtime.config.db_dir,
            &runtime.config.wechat_process,
            Duration::from_secs(args.timeout),
            args.max_mib * 1024 * 1024,
        )?
    };
    let result = (|| -> Result<Value> {
        ensure!(
            !cancelled.load(Ordering::SeqCst),
            "图片取钥已取消，未更新配置"
        );
        let staged_sample = sample.map(|sample| sample.stage(&material)).transpose()?;
        if !args.no_save {
            guard.verify_replaceable_file(&runtime.config_path)?;
            ensure!(
                read_config(&runtime.config_path)?.as_slice() == original.as_slice(),
                "配置在扫描期间变化，拒绝覆盖其他修改"
            );
            ensure!(
                !cancelled.load(Ordering::SeqCst),
                "图片取钥已取消，未更新配置"
            );
            crate::service::worker_keys::commit_sync(
                runtime,
                vec![crate::service::worker_keys::MaterialChange::Image {
                    aes: material.aes_key,
                    xor: material.xor_key,
                }],
            )?;
        }
        let sample_report = staged_sample
            .map(|sample| sample.publish())
            .transpose()
            .with_context(|| {
                if args.no_save {
                    "样本发布失败，未更新配置"
                } else {
                    "加密图片密钥已更新，但样本发布失败"
                }
            })?;
        Ok(json!({"engine":"rust", "account_id":runtime.id,
            "aes_template_verified":true,
            "inference_mode":if args.offline {"offline"} else {"process_memory"},
            "xor_policy":if args.offline {"thumbnail_tail_vote"} else {"sample_vote_or_0x88"},
            "config_updated":false, "key_store_updated":!args.no_save, "keys_redacted":true, "sample":sample_report}))
    })();
    material.aes_key.zeroize();
    result
}

pub use crate::service::operation_requests::image_keys::MonitorArgs;

pub fn cmd_monitor(args: MonitorArgs) -> Result<()> {
    args.validate_request()?;
    ensure!(args.authorize_memory_scan, "图片密钥监控需要明确授权");
    let runtime = Arc::new(RuntimeContext::load()?);
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    executor.block_on(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(args.timeout);
        let signal = tokio::signal::ctrl_c();
        tokio::pin!(signal);
        let mut rounds = 0u64;
        loop {
            ensure!(tokio::time::Instant::now() < deadline, "图片密钥监控超时，未找到已验证密钥");
            let scan = Args {
                sample: args.sample.clone(),
                authorize_memory_scan: true,
                offline: false,
                no_save: args.no_save,
                timeout: args.scan_seconds,
                max_mib: args.max_mib,
            };
            let account = Arc::clone(&runtime);
            let cancellation = Arc::clone(&cancelled);
            let mut worker = tokio::task::spawn_blocking(move || extract_cancellable(&account, scan, &cancellation, true));
            let outcome = tokio::select! {
                biased;
                signal_result = &mut signal => {
                    cancelled.store(true, Ordering::SeqCst);
                    let _ = worker.await;
                    signal_result.context("监听退出信号失败")?;
                    println!("{}", json!({"engine":"rust", "account_id":runtime.id, "cancelled":true, "rounds":rounds}));
                    return Ok(());
                }
                _ = tokio::time::sleep_until(deadline) => {
                    cancelled.store(true, Ordering::SeqCst);
                    let _ = worker.await;
                    anyhow::bail!("图片密钥监控超时，扫描资源已回收");
                }
                result = &mut worker => result.context("图片扫描工作线程异常")?,
            };
            rounds += 1;
            match outcome {
                Ok(mut report) => {
                    report["rounds"] = rounds.into();
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    return Ok(());
                }
                Err(error) if error.is::<crate::attachment::image_key::NoImageKeyFound>() => {
                    eprintln!("第 {rounds} 轮未找到已验证密钥，等待下一轮");
                }
                Err(error) => return Err(error),
            }
            tokio::select! {
                biased;
                signal_result = &mut signal => {
                    signal_result.context("监听退出信号失败")?;
                    println!("{}", json!({"engine":"rust", "account_id":runtime.id, "cancelled":true, "rounds":rounds}));
                    return Ok(());
                }
                _ = tokio::time::sleep_until(deadline) => anyhow::bail!("图片密钥监控超时，未找到已验证密钥"),
                _ = tokio::time::sleep(Duration::from_millis(args.interval_ms)) => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[derive(Parser)]
    struct Invocation {
        #[command(flatten)]
        args: crate::cli::operation_args::image_keys::Args,
    }

    #[test]
    fn offline_and_memory_modes_require_exactly_one_choice() {
        assert!(Invocation::try_parse_from(["image-key"]).is_err());
        assert!(
            Invocation::try_parse_from(["image-key", "--offline", "--authorize-memory-scan",])
                .is_err()
        );
        let offline = Invocation::try_parse_from(["image-key", "--offline"])
            .unwrap()
            .args;
        assert!(offline.offline && !offline.authorize_memory_scan);
        let memory = Invocation::try_parse_from(["image-key", "--authorize-memory-scan"])
            .unwrap()
            .args;
        assert!(!memory.offline && memory.authorize_memory_scan);
    }

    #[test]
    fn offline_mode_keeps_sample_pair_and_budget_validation() {
        assert!(Invocation::try_parse_from([
            "image-key",
            "--offline",
            "--sample-input",
            "example.dat",
        ])
        .is_err());
        assert!(Invocation::try_parse_from(["image-key", "--offline", "--timeout", "0",]).is_err());
        assert!(
            Invocation::try_parse_from(["image-key", "--offline", "--no-save",])
                .unwrap()
                .args
                .no_save
        );
    }
}
