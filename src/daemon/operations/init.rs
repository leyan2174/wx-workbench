use anyhow::{Context, Result};
use serde_json::json;

use crate::config;
use crate::scanner;
use crate::toolkit::setup::{self, ConfigDocument, Snapshot};

pub fn cmd_init(
    force: bool,
    db_dir_override: Option<String>,
    provider: scanner::KeyProvider,
    restart: bool,
    executable: Option<std::path::PathBuf>,
    timeout: u64,
) -> Result<()> {
    anyhow::ensure!(
        !restart || provider == scanner::KeyProvider::Account,
        "--restart-wechat 仅用于 --key-provider account"
    );
    anyhow::ensure!(
        provider != scanner::KeyProvider::Account || (force && restart),
        "账号级捕获需要 --force --key-provider account --restart-wechat"
    );
    anyhow::ensure!(
        executable.is_none() || provider == scanner::KeyProvider::Account,
        "--wechat-exe 仅用于 --key-provider account"
    );
    // 查找 config.json
    let config_path = find_or_create_config_path()?;
    // 坏 JSON、非对象及错误路径字段必须在扫描前失败，不能当作空配置覆盖。
    let document = ConfigDocument::load(&config_path)?;

    // Step 1: 检测 db_dir
    println!("检测微信数据目录...");
    let configured_db_dir = document.configured_db()?;
    let db_dir = if let Some(db_dir) = db_dir_override {
        let path = std::path::absolute(std::path::PathBuf::from(db_dir))?;
        crate::attachment::local_files::HostOutputGuard::new(&path)
            .context("指定的 db_storage 目录不存在或不安全")?;
        path
    } else if let Some(db_dir) = configured_db_dir {
        let path = db_dir;
        crate::attachment::local_files::HostOutputGuard::new(&path)
            .context("配置中的账号目录不存在或不安全，请核对选中配置")?;
        path
    } else {
        config::auto_detect_db_dir().with_context(|| format!(
            "未能自动检测到微信数据目录\n\
             请编辑配置文件并填写 db_dir 字段:\n  \
             {}\n\
             （文件不存在则首次保存后自动创建；db_dir 示例: <data_root>\\xwechat_files\\<wxid>\\db_storage）",
            config_path.display()
        ))?
    };
    let db_dir = std::path::absolute(db_dir)?;
    let cfg = document.with_db(&db_dir)?;
    let paths = document.validate_targets(&cfg)?;
    // 自定义 keys_file 的检查与写入使用同一解析结果；--force 也不允许串号覆盖。
    if !force && document.snapshot.existed() && setup::exists(&paths.keys_file)? {
        println!("已初始化，数据目录: {}", db_dir.display());
        println!("如需重新扫描密钥，使用 --force");
        return Ok(());
    }
    println!("找到数据目录: {}", db_dir.display());
    let _lock = document.lock()?;
    let db_guard = crate::attachment::local_files::HostOutputGuard::new(&db_dir)?;
    let keys_snapshot = Snapshot::capture(&paths.keys_file)?;

    // Step 2: 扫描密钥
    println!("扫描加密密钥...");
    let process_name = cfg
        .get("wechat_process")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Weixin.exe");
    let account_key_file = paths.account_key_file.clone();
    let entries = scanner::scan_with_provider(
        &db_dir,
        process_name,
        provider,
        restart,
        executable.as_deref(),
        timeout,
        &account_key_file,
    )?;
    if entries.is_empty() {
        anyhow::bail!(
            "未验证到任何数据库密钥，已保留现有密钥 JSON；请确认微信已登录且数据目录正确"
        );
    }
    db_guard.verify()?;
    document.snapshot.verify()?;

    // Step 3: 同卷原子保存配置声明的密钥路径，不回显密钥内容。
    let mut keys_json = serde_json::Map::new();
    for entry in &entries {
        keys_json.insert(
            entry.db_name.clone(),
            json!({
                "enc_key": entry.enc_key,
            }),
        );
    }
    let mut key_protection = paths.protected.clone();
    key_protection.extend([config_path.clone(), account_key_file.clone()]);
    keys_snapshot
        .write_json(&serde_json::Value::Object(keys_json), &key_protection)
        .context("原子保存选中账号密钥 JSON 失败")?;
    println!("成功提取 {} 个数据库密钥", entries.len());
    println!("密钥已保存: {}", paths.keys_file.display());

    // Step 4: 使用原始快照保留未知字段，拒绝覆盖扫描期间的其他配置修改。
    let mut config_protection = paths.protected;
    config_protection.extend([paths.keys_file.clone(), account_key_file]);
    document
        .snapshot
        .write_json(&cfg, &config_protection)
        .with_context(|| {
            format!(
                "密钥已原子保存至 {}，但配置提交失败；未报告初始化成功，请检查配置后重试",
                paths.keys_file.display()
            )
        })?;
    println!("配置已保存: {}", config_path.display());

    // The supervising daemon invalidates query state after the worker completes.
    // Stopping it here would also terminate this operation's Windows Job.

    println!("初始化完成，可以使用 wx sessions / wx history 等命令了");

    Ok(())
}

fn find_or_create_config_path() -> Result<std::path::PathBuf> {
    // 首次创建与正常读取共用同一定位函数，避免写到了读取器不会选择的位置。
    Ok(std::path::absolute(config::find_config_file()?)?)
}
