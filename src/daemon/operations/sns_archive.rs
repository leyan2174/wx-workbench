//! standalone decrypt_sns.py 的账号固定宿主，不加载时间线或自动发现其他账号。
use crate::{
    adapters::wechat::moments::cache::{CacheKeys, CacheLimits, CacheRoots},
    runtime::RuntimeContext,
    toolkit::sns::archive::{self, ArchiveOptions, ArchiveReport},
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub use crate::service::operation_requests::sns_archive::Args;

fn resolve(base: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(!path.as_os_str().is_empty(), "路径不能为空");
    // 配置中的相对路径固定相对于选中配置，不随 vendor 或后续工作目录变化。
    Ok(if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    })
}

fn keys(runtime: &RuntimeContext) -> Result<CacheKeys> {
    let material = zeroize::Zeroizing::new(
        crate::key_store::Store::for_runtime(runtime)?
            .load()?
            .image_material(),
    );
    Ok(CacheKeys {
        image_aes_key: material.0,
        image_xor_key: material.1,
    })
}

/// 调用方传入已固定的 RuntimeContext 和同一配置文件的扩展字段。
/// 返回完整报告，由宿主决定显示方式；这里不重复发现配置、不启动 daemon。
pub fn export_for(runtime: &RuntimeContext, raw: &Value, args: Args) -> Result<ArchiveReport> {
    ensure!(raw.is_object(), "选中账号配置必须为 JSON 对象");
    let base = runtime.config_path.parent().context("选中配置缺少父目录")?;
    let db = resolve(base, &runtime.config.db_dir)?;
    let account = crate::adapters::wechat::moments::cache::account_root(&db)?;
    let account_name = account
        .file_name()
        .and_then(|n| n.to_str())
        .context("选中账号目录名无效")?;
    let legacy = match raw.get("wechat_files_dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(resolve(base, Path::new(s))?),
        Some(_) => anyhow::bail!("wechat_files_dir 必须为路径字符串"),
    };
    // 与 config.py 一样忽略被无条件覆盖的 sns_cache_dir/xwechat_cache_dir/output_base_dir。
    // 唯一不复刻的发现逻辑是 Documents/WeChat Files 下的账号前缀模糊匹配。
    let roots = CacheRoots::for_account(account, legacy.as_deref());
    let output_base = match args.output_dir {
        Some(path) => std::path::absolute(path)?,
        None => base.join("wechat_files").join(account_name),
    };
    let options = ArchiveOptions {
        output: output_base.join("朋友圈图片"),
        source_id: runtime.id.clone(),
        account_name: account_name.into(),
        protected_inputs: vec![
            runtime.config_path.clone(),
            resolve(base, &runtime.config.keys_file)?,
            account.into(),
            resolve(base, &runtime.config.decrypted_dir)?,
            runtime.directory.clone(),
        ],
        adopt_existing: args.adopt_existing,
        limits: CacheLimits::default(),
    };
    let keys = keys(runtime)?;
    archive::export(&roots, &keys, &options)
}

pub fn cmd(args: Args) -> Result<()> {
    let runtime = RuntimeContext::load().map_err(|_| anyhow::anyhow!("无法加载选中账号配置"))?;
    // 固定文件句柄，读扩展字段期间禁止配置写入或替换；绝不回显配置或密钥。
    let guard = crate::attachment::local_files::HostOutputGuard::new(
        runtime.config_path.parent().context("选中配置缺少父目录")?,
    )?;
    guard.verify_replaceable_file(&runtime.config_path)?;
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(&runtime.config_path)
        .map_err(|_| anyhow::anyhow!("无法固定选中账号配置"))?;
    // RuntimeContext 加载与固定句柄之间若切换了账号配置，终止而非混用密钥。
    let pinned = crate::config::load_config_at(&runtime.config_path)
        .map_err(|_| anyhow::anyhow!("无法核验选中账号配置"))?;
    ensure!(
        pinned.db_dir == runtime.config.db_dir
            && pinned.keys_file == runtime.config.keys_file
            && pinned.decrypted_dir == runtime.config.decrypted_dir,
        "选中账号配置已变化，请重新运行"
    );
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    (&file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("无法读取选中账号配置"))?;
    ensure!(bytes.len() <= 1024 * 1024, "选中配置超过大小限制");
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("选中账号配置 JSON 无效"))?;
    let report = export_for(&runtime, &raw, args)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "engine": "rust", "archive": report }))?
    );
    // 缺失缓存兼容旧脚本成功退出；实际失败或不完整枚举在完整报告后返回非零。
    ensure!(!report.incomplete, "朋友圈缓存未完整枚举，详见归档报告");
    ensure!(
        report.counts.failed == 0,
        "{} 个缓存文件归档失败，详见归档报告",
        report.counts.failed
    );
    Ok(())
}
