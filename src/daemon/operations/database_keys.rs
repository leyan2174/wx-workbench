//! 固定账号的显式数据库取钥入口；不写配置、不重启微信、不输出密钥材料。
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use zeroize::Zeroize;

use super::database_key_validation::validate_keys;
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};

pub use crate::service::operation_requests::database_keys::Args;

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
    let config_pin = crate::service::config_pin::ConfigPin::new(runtime)?;
    let current = RuntimeContext::load()?;
    ensure!(
        current.same_account(runtime)?,
        "选中账号配置发生变化，未开始扫描"
    );
    let target = runtime
        .config
        .key_store
        .as_deref()
        .context("当前账号未配置正式密钥存储")?;
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
    let before = crate::service::worker_keys::expected_revision(runtime)?;
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
        config_pin.verify(runtime)?;
        output_guard.verify_replaceable_file(target)?;
        ensure!(
            crate::service::worker_keys::expected_revision(runtime)? == before,
            "扫描期间密钥代际发生变化，拒绝覆盖并发修改"
        );
        let count = keys.len();
        crate::service::worker_keys::commit_sync(
            runtime,
            vec![crate::service::worker_keys::MaterialChange::Databases(
                std::mem::take(&mut keys),
            )],
        )?;
        Ok(
            json!({"engine":"rust", "account_id":runtime.id, "databases":count,
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
