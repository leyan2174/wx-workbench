pub mod cache;
pub mod query;
pub mod server;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::config;

/// daemon 入口
///
/// 当 WX_DAEMON_MODE 环境变量设置时，main() 调用此函数
pub fn run() {
    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    if let Err(e) = rt.block_on(async_run()) {
        eprintln!("[daemon] 启动失败: {}", e);
        std::process::exit(1);
    }
}

async fn async_run() -> Result<()> {
    // 确保工作目录存在
    let cli_dir = config::cli_dir();
    tokio::fs::create_dir_all(&cli_dir).await?;
    tokio::fs::create_dir_all(config::cache_dir()).await?;

    // 写 PID 文件
    let pid = std::process::id();
    tokio::fs::write(config::pid_path(), pid.to_string()).await?;

    // 注册 SIGTERM / SIGINT 处理
    setup_signal_handler().await;

    eprintln!("[daemon] wx-daemon 启动 (PID {})", pid);

    // 加载配置
    let cfg = config::load_config()?;
    eprintln!("[daemon] DB_DIR: {}", cfg.db_dir.display());

    // 加载密钥
    let keys_content = tokio::fs::read_to_string(&cfg.keys_file).await
        .map_err(|e| anyhow::anyhow!("读取密钥文件 {:?} 失败: {}", cfg.keys_file, e))?;
    let keys_raw: serde_json::Value = serde_json::from_str(&keys_content)?;
    let all_keys = extract_keys(&keys_raw);
    eprintln!("[daemon] 密钥数量: {}", all_keys.len());

    // 初始化 DbCache
    let db = Arc::new(cache::DbCache::new(cfg.db_dir.clone(), all_keys.clone()).await?);

    // 收集消息 DB 列表
    let msg_db_keys: Vec<String> = all_keys.keys()
        .filter(|k| {
            let k = k.replace('\\', "/");
            k.contains("message/message_") && k.ends_with(".db")
        })
        .cloned()
        .collect();

    // 预热：加载联系人 + 解密 session.db
    eprintln!("[daemon] 预热...");
    let names_raw = query::load_names(&*db).await.unwrap_or_else(|e| {
        eprintln!("[daemon] 加载联系人失败: {}", e);
        query::Names {
            map: HashMap::new(),
            md5_to_uname: HashMap::new(),
            msg_db_keys: Vec::new(),
        }
    });
    let mut names = names_raw;
    names.msg_db_keys = msg_db_keys;

    let _ = db.get("session/session.db").await;
    eprintln!("[daemon] 预热完成，联系人 {} 个", names.map.len());

    let names_arc = Arc::new(std::sync::RwLock::new(names));

    // 启动 WAL watcher
    let (watch_tx, _) = broadcast::channel::<crate::ipc::WatchEvent>(500);
    let session_wal = cfg.db_dir.join("session").join("session.db-wal");

    // SAFETY: 我们确保 db 和 names_arc 在 daemon 生命周期内有效
    // 使用 Arc 传递引用避免 'static 问题
    let db_arc = Arc::clone(&db);
    let names_arc2 = Arc::clone(&names_arc);
    let tx_clone = watch_tx.clone();
    let session_wal2 = session_wal.clone();
    tokio::spawn(async move {
        run_watcher(db_arc, names_arc2, tx_clone, session_wal2).await;
    });

    // 启动 IPC server（阻塞）
    server::serve(Arc::clone(&db), Arc::clone(&names_arc), watch_tx).await?;

    Ok(())
}

async fn run_watcher(
    db: Arc<cache::DbCache>,
    names: Arc<std::sync::RwLock<query::Names>>,
    tx: broadcast::Sender<crate::ipc::WatchEvent>,
    session_wal: PathBuf,
) {
    use std::collections::HashMap;
    use std::time::Duration;
    use crate::ipc::WatchEvent;

    let mut last_mtime = 0u64;
    let mut last_ts: HashMap<String, i64> = HashMap::new();
    let mut initialized = false;

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        if tx.receiver_count() == 0 {
            continue;
        }

        let wal_mtime = match mtime_nanos(&session_wal) {
            0 => continue,
            m => m,
        };
        if wal_mtime == last_mtime {
            continue;
        }
        last_mtime = wal_mtime;

        let path = match db.get("session/session.db").await {
            Ok(Some(p)) => p,
            _ => continue,
        };

        let path2 = path.clone();
        let rows: Vec<(String, Vec<u8>, i64, i64, String)> = match tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&path2)?;
            let mut stmt = conn.prepare(
                "SELECT username, summary, last_timestamp, last_msg_type, last_msg_sender
                 FROM SessionTable WHERE last_timestamp > 0
                 ORDER BY last_timestamp DESC LIMIT 50"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1).unwrap_or_default(),
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3).unwrap_or(0),
                    row.get::<_, String>(4).unwrap_or_default(),
                ))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, anyhow::Error>(rows)
        }).await {
            Ok(Ok(r)) => r,
            _ => continue,
        };

        let names_guard = match names.read() {
            Ok(g) => g,
            Err(_) => continue,
        };

        for (username, summary_bytes, ts, msg_type, sender) in &rows {
            if !initialized {
                last_ts.insert(username.clone(), *ts);
                continue;
            }
            let prev_ts = last_ts.get(username).copied().unwrap_or(0);
            if *ts <= prev_ts {
                continue;
            }
            last_ts.insert(username.clone(), *ts);

            let display = names_guard.display(username);
            let is_group = username.contains("@chatroom");
            let summary = decompress_or_str(summary_bytes);
            let summary = if summary.contains(":\n") {
                summary.splitn(2, ":\n").nth(1).unwrap_or(&summary).to_string()
            } else {
                summary
            };
            let sender_display = if !sender.is_empty() {
                names_guard.map.get(sender).cloned().unwrap_or_else(|| sender.clone())
            } else {
                String::new()
            };

            let event = WatchEvent {
                event: "message".into(),
                time: Some(fmt_hhmm(*ts)),
                chat: Some(display),
                username: Some(username.clone()),
                is_group: Some(is_group),
                sender: Some(sender_display),
                content: Some(summary),
                msg_type: Some(query::fmt_type(*msg_type)),
                timestamp: Some(*ts),
            };
            let _ = tx.send(event);
        }

        if !initialized {
            initialized = true;
        }
    }
}

fn mtime_nanos(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64)
        .unwrap_or(0)
}

fn decompress_or_str(data: &[u8]) -> String {
    if data.is_empty() { return String::new(); }
    if let Ok(dec) = zstd::decode_all(data) {
        if let Ok(s) = String::from_utf8(dec) { return s; }
    }
    String::from_utf8_lossy(data).into_owned()
}

fn fmt_hhmm(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// 从 all_keys.json 提取 rel_key -> enc_key 映射
///
/// 兼容两种格式：
/// - `{ "rel/path.db": { "enc_key": "hex" } }`（Python 版原生格式）
/// - `{ "rel/path.db": "hex" }`（简化格式）
fn extract_keys(json: &serde_json::Value) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if let Some(obj) = json.as_object() {
        for (k, v) in obj {
            if k.starts_with('_') { continue; }
            let enc_key = if let Some(s) = v.as_str() {
                s.to_string()
            } else if let Some(obj2) = v.as_object() {
                obj2.get("enc_key")
                    .and_then(|e| e.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                continue;
            };
            if !enc_key.is_empty() {
                // 统一路径分隔符
                let rel = k.replace('\\', "/");
                result.insert(rel, enc_key);
            }
        }
    }
    result
}

/// 设置信号处理（Unix: SIGTERM/SIGINT）
async fn setup_signal_handler() {
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("无法监听 SIGTERM");
        let mut int = signal(SignalKind::interrupt()).expect("无法监听 SIGINT");
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
        cleanup_and_exit();
    });
}

fn cleanup_and_exit() {
    let _ = std::fs::remove_file(config::sock_path());
    let _ = std::fs::remove_file(config::pid_path());
    std::process::exit(0);
}
