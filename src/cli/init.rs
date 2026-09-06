use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;

use crate::config;
use crate::scanner;

pub fn cmd_init(
    force: bool, db_dir_override: Option<String>, provider: scanner::KeyProvider,
    restart: bool, executable: Option<std::path::PathBuf>, timeout: u64,
) -> Result<()> {
    anyhow::ensure!(!restart || provider == scanner::KeyProvider::Account,
        "--restart-wechat 仅用于 --key-provider account");
    anyhow::ensure!(provider != scanner::KeyProvider::Account || (force && restart),
        "账号级捕获需要 --force --key-provider account --restart-wechat");
    anyhow::ensure!(executable.is_none() || provider == scanner::KeyProvider::Account,
        "--wechat-exe 仅用于 --key-provider account");
    // 查找 config.json
    let config_path = find_or_create_config_path();

    // 检查是否已初始化
    if !force && config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&content) {
                let db_dir = cfg.get("db_dir").and_then(|v| v.as_str()).unwrap_or("");
                let keys_file = cfg.get("keys_file").and_then(|v| v.as_str()).unwrap_or("all_keys.json");
                let keys_path = if std::path::Path::new(keys_file).is_absolute() {
                    std::path::PathBuf::from(keys_file)
                } else {
                    config_path.parent().unwrap_or(std::path::Path::new("."))
                        .join(keys_file)
                };
                if !db_dir.is_empty() && !db_dir.contains("your_wxid")
                    && std::path::Path::new(db_dir).exists()
                    && keys_path.exists()
                {
                    println!("已初始化，数据目录: {}", db_dir);
                    println!("如需重新扫描密钥，使用 --force");
                    return Ok(());
                }
            }
        }
    }

    // Step 1: 检测 db_dir
    println!("检测微信数据目录...");
    let scan_config = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
    let configured_db_dir = scan_config.as_ref()
        .and_then(|cfg| cfg.get("db_dir"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty() && !value.contains("your_wxid"));
    let db_dir = if let Some(db_dir) = db_dir_override {
        let path = std::path::PathBuf::from(db_dir);
        if !path.is_dir() {
            anyhow::bail!("指定的 db_storage 目录不存在: {}", path.display());
        }
        path
    } else if let Some(db_dir) = configured_db_dir {
        let path = std::path::PathBuf::from(db_dir);
        let path = if path.is_absolute() { path } else {
            config_path.parent().unwrap_or(std::path::Path::new(".")).join(path)
        };
        anyhow::ensure!(path.is_dir(), "配置中的账号目录不存在，请使用 --db-dir 指定");
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
    println!("找到数据目录: {}", db_dir.display());

    // Step 2: 扫描密钥
    println!("扫描加密密钥...");
    let process_name = scan_config
        .as_ref()
        .and_then(|cfg| cfg.get("wechat_process"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Weixin.exe");
    let account_key_file = config_path.parent().unwrap_or(std::path::Path::new("."))
        .join("account_key.dpapi");
    let entries = scanner::scan_with_provider(&db_dir, process_name, provider,
        restart, executable.as_deref(), timeout, &account_key_file)?;
    if entries.is_empty() {
        anyhow::bail!(
            "未验证到任何数据库密钥，已保留现有 all_keys.json；请确认微信已登录且数据目录正确"
        );
    }

    // 确保父目录存在（如 ~/.wx-cli/），必须在任何写入之前
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    // Step 3: 保存 all_keys.json
    let keys_file_path = config_path.parent()
        .unwrap_or(std::path::Path::new("."))
        .join("all_keys.json");

    let mut keys_json = serde_json::Map::new();
    for entry in &entries {
        keys_json.insert(entry.db_name.clone(), json!({
            "enc_key": entry.enc_key,
        }));
    }
    std::fs::write(&keys_file_path, serde_json::to_string_pretty(&keys_json)?)
        .context("写入 all_keys.json 失败")?;
    println!("成功提取 {} 个数据库密钥", entries.len());
    println!("密钥已保存: {}", keys_file_path.display());

    // Step 4: 保存 config.json
    let mut cfg = HashMap::new();
    // 读取已有配置
    if config_path.exists() {
        if let Ok(c) = std::fs::read_to_string(&config_path) {
            if let Ok(v) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&c) {
                for (k, val) in v {
                    cfg.insert(k, val);
                }
            }
        }
    }
    cfg.insert("db_dir".into(), json!(db_dir.to_string_lossy()));
    cfg.entry("keys_file".into()).or_insert_with(|| json!("all_keys.json"));
    cfg.entry("decrypted_dir".into()).or_insert_with(|| json!("decrypted"));

    std::fs::write(&config_path, serde_json::to_string_pretty(&cfg)?)
        .context("写入 config.json 失败")?;
    println!("配置已保存: {}", config_path.display());

    // init 之后必须停掉旧 daemon（它用的是旧 config），下次调用会自动重启
    let _ = crate::cli::transport::stop_daemon();

    println!("初始化完成，可以使用 wx sessions / wx history 等命令了");

    Ok(())
}

fn find_or_create_config_path() -> std::path::PathBuf {
    // 如果当前工作目录或可执行文件目录已有 config.json，沿用它（支持便携模式）
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("config.json");
        if p.exists() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("config.json");
            if p.exists() {
                return p;
            }
        }
    }
    // 默认写入 ~/.wx-cli/config.json（与 load_config 的最终查找路径保持一致）
    config::cli_dir().join("config.json")
}
