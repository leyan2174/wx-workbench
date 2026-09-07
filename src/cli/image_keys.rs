//! 独立图片密钥命令；只更新已固定账号的配置，不在终端输出密钥材料。
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub sample: super::image_key_sample::SampleArgs,
    /// 明确授权读取当前微信进程内存
    #[arg(long, required_unless_present = "offline", conflicts_with = "offline")]
    pub authorize_memory_scan: bool,
    /// 仅从当前账号目录及图片缓存推导密钥，不读取进程内存
    #[arg(long)]
    pub offline: bool,
    /// 仅提取和验证，不更新配置
    #[arg(long)]
    pub no_save: bool,
    /// 离线推导或全部候选进程共用的时间预算（秒）
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub timeout: u64,
    /// 图片缓存与候选进程共用的读取预算（MiB）
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u64).range(1..=32768))]
    pub max_mib: u64,
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
    let mut config: Value = serde_json::from_slice(&original).context("账号配置 JSON 无效")?;
    ensure!(config.is_object(), "账号配置必须为对象");
    let current = RuntimeContext::load()?;
    ensure!(
        current.id == runtime.id
            && current.config_path == runtime.config_path
            && current.config.db_dir == runtime.config.db_dir
            && current.config.keys_file == runtime.config.keys_file
            && current.config.decrypted_dir == runtime.config.decrypted_dir
            && current.config.wechat_process == runtime.config.wechat_process,
        "选中账号配置发生变化，未开始扫描"
    );
    if reuse_existing {
        if let Some(value) = config
            .get("image_aes_key")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let key = Zeroizing::new(crate::toolkit::parse_image_aes(value)?);
            let valid = crate::attachment::image_key::windows::validate_existing_for_db_dir(
                &runtime.config.db_dir,
                &key,
                Duration::from_secs(args.timeout),
                args.max_mib * 1024 * 1024,
            )?;
            ensure!(!cancelled.load(Ordering::SeqCst), "图片取钥已取消");
            if valid {
                guard.verify_replaceable_file(&runtime.config_path)?;
                ensure!(
                    read_config(&runtime.config_path)?.as_slice() == original.as_slice(),
                    "验证期间账号配置发生变化"
                );
                let sample_report = if let Some(sample) = sample {
                    let xor = config.get("image_xor_key").map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    });
                    let mut material = crate::attachment::image_key::ImageKeyMaterial {
                        aes_key: *key,
                        xor_key: xor
                            .as_deref()
                            .map(crate::toolkit::parse_image_xor)
                            .transpose()?
                            .unwrap_or(0x88),
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
                    "existing_key_valid":true, "config_updated":false, "keys_redacted":true, "sample":sample_report}));
            }
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
            let key = Zeroizing::new(
                String::from_utf8(material.aes_key.to_vec())
                    .context("图片 AES 材料不是 ASCII 文本")?,
            );
            config["image_aes_key"] = Value::String(key.to_string());
            config["image_xor_key"] = material.xor_key.into();
            let encoded = Zeroizing::new(serde_json::to_vec_pretty(&config)?);
            if let Some(Value::String(secret)) = config.get_mut("image_aes_key") {
                secret.zeroize();
            }
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&encoded)?;
            temporary.write_all(b"\n")?;
            temporary.as_file().sync_all()?;
            fs::set_permissions(
                temporary.path(),
                fs::metadata(&runtime.config_path)?.permissions(),
            )?;
            guard.verify_replaceable_file(&runtime.config_path)?;
            ensure!(
                read_config(&runtime.config_path)?.as_slice() == original.as_slice(),
                "配置在扫描期间变化，拒绝覆盖其他修改"
            );
            ensure!(
                !cancelled.load(Ordering::SeqCst),
                "图片取钥已取消，未更新配置"
            );
            temporary
                .persist(&runtime.config_path)
                .map_err(|error| error.error)
                .context("图片密钥配置更新失败，原配置已保留")?;
        }
        let sample_report = staged_sample
            .map(|sample| sample.publish())
            .transpose()
            .with_context(|| {
                if args.no_save {
                    "样本发布失败，未更新配置"
                } else {
                    "图片密钥配置已更新，但样本发布失败"
                }
            })?;
        Ok(json!({"engine":"rust", "account_id":runtime.id,
            "aes_template_verified":true,
            "inference_mode":if args.offline {"offline"} else {"process_memory"},
            "xor_policy":if args.offline {"thumbnail_tail_vote"} else {"sample_vote_or_0x88"},
            "config_updated":!args.no_save, "keys_redacted":true, "sample":sample_report}))
    })();
    material.aes_key.zeroize();
    result
}

#[derive(Debug, clap::Args)]
pub struct MonitorArgs {
    #[command(flatten)]
    sample: super::image_key_sample::SampleArgs,
    /// 明确授权在监控期间重复读取当前微信进程内存
    #[arg(long, required = true)]
    authorize_memory_scan: bool,
    /// 找到后仅验证，不更新配置
    #[arg(long)]
    no_save: bool,
    /// 单轮扫描上限（秒）；取消时等待当前有界扫描回收资源
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=30))]
    scan_seconds: u64,
    /// 两轮扫描之间的等待时间（毫秒）
    #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(100..=60000))]
    interval_ms: u64,
    /// 整个监控的时间上限（秒）
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=86400))]
    timeout: u64,
    /// 每轮最多读取的内存（MiB）
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u64).range(1..=32768))]
    max_mib: u64,
}

pub fn cmd_monitor(args: MonitorArgs) -> Result<()> {
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
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Invocation {
        #[command(flatten)]
        args: Args,
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
