//! 固定账号的显式数据库取钥入口；不写配置、不重启微信、不输出密钥材料。
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, fs::OpenOptions};
use zeroize::Zeroize;

use super::toolkit_run_prepare::{save_keys, snapshot_keys, validate_keys};
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 明确授权只读扫描配置中微信进程的内存
    #[arg(long, required = true)]
    pub authorize_memory_scan: bool,
}

pub fn cmd(args: Args) -> Result<()> {
    ensure!(
        args.authorize_memory_scan,
        "数据库取钥需要 --authorize-memory-scan 明确授权"
    );
    let runtime = RuntimeContext::load()?;
    println!("{}", serde_json::to_string_pretty(&extract_for(&runtime)?)?);
    Ok(())
}

fn extract_for(runtime: &RuntimeContext) -> Result<Value> {
    if let Some(expected) = std::env::var_os("WX_CLI_EXPECTED_RUNTIME") {
        ensure!(
            expected.to_str() == Some(runtime.id.as_str()),
            "当前账号与父进程固定账号不一致，未开始扫描"
        );
    }
    let config_parent = runtime.config_path.parent().context("配置缺少父目录")?;
    let config_guard = HostOutputGuard::new(config_parent)?;
    config_guard.verify_replaceable_file(&runtime.config_path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let _config_pin = options
        .open(&runtime.config_path)
        .context("无法锁定当前账号配置")?;
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
    let target = &runtime.config.keys_file;
    let parent = target.parent().context("密钥输出缺少父目录")?;
    let output_guard = HostOutputGuard::new(parent)?;
    output_guard.verify_replaceable_file(target)?;
    let resolved = parent
        .canonicalize()?
        .join(target.file_name().context("密钥输出缺少文件名")?);
    let account = runtime
        .config
        .db_dir
        .parent()
        .context("账号数据目录缺少父目录")?
        .canonicalize()?;
    ensure!(
        !resolved.ancestors().any(|ancestor| ancestor
            .as_os_str()
            .eq_ignore_ascii_case(account.as_os_str())),
        "密钥输出不得位于原始账号数据目录中"
    );
    ensure!(
        !resolved
            .as_os_str()
            .eq_ignore_ascii_case(runtime.config_path.canonicalize()?.as_os_str()),
        "密钥输出不得覆盖配置文件"
    );
    if target.exists() {
        ensure!(
            !same_file::is_same_file(target, &runtime.config_path)?,
            "密钥输出不得覆盖配置文件别名"
        );
    }
    let before = snapshot_keys(target)?;
    let source_guard = HostOutputGuard::new(&runtime.config.db_dir)?;
    let mut entries =
        crate::scanner::scan_keys_checked(&runtime.config.db_dir, &runtime.config.wechat_process)?;
    let mut keys = HashMap::new();
    let result = (|| -> Result<Value> {
        for entry in &mut entries {
            ensure!(
                !keys.contains_key(&entry.db_name),
                "扫描结果包含重复数据库路径"
            );
            keys.insert(entry.db_name.clone(), std::mem::take(&mut entry.enc_key));
        }
        validate_keys(runtime, &keys)?;
        source_guard.verify()?;
        config_guard.verify()?;
        output_guard.verify_replaceable_file(target)?;
        ensure!(
            snapshot_keys(target)? == before,
            "扫描期间密钥文件发生变化，拒绝覆盖并发修改"
        );
        save_keys(runtime, &keys)?;
        Ok(
            json!({"engine":"rust", "account_id":runtime.id, "databases":keys.len(),
            "keys_updated":true, "keys_redacted":true, "config_updated":false}),
        )
    })();
    for key in keys.values_mut() {
        key.zeroize();
    }
    for entry in &mut entries {
        entry.enc_key.zeroize();
    }
    result
}
