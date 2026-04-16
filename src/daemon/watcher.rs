use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::broadcast;

use super::cache::DbCache;
use super::query::{fmt_type, Names};
use crate::ipc::WatchEvent;

/// 启动 WAL 变化监听 task
///
/// 每 500ms 检测 session.db-wal 的 mtime，有变化时重新读 session.db，
/// 找到 timestamp 更新的行，broadcast 到所有 watch 客户端
#[allow(dead_code)]
pub async fn start_watcher(
    db: &'static DbCache,
    names_ref: &'static std::sync::RwLock<Names>,
    tx: broadcast::Sender<WatchEvent>,
    session_wal_path: PathBuf,
) {
    tokio::spawn(async move {
        let mut last_mtime = 0.0f64;
        let mut last_ts: HashMap<String, i64> = HashMap::new();
        let mut initialized = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            // 如果没有订阅者，跳过
            if tx.receiver_count() == 0 {
                continue;
            }

            let wal_mtime = match mtime_f64(&session_wal_path) {
                Some(m) => m,
                None => continue,
            };

            if (wal_mtime - last_mtime).abs() < 0.001 {
                continue;
            }
            last_mtime = wal_mtime;

            // 重新解密 session.db
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
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok::<_, anyhow::Error>(rows)
            }).await {
                Ok(Ok(r)) => r,
                _ => continue,
            };

            let names_guard = names_ref.read().expect("names lock poisoned");

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
                    time: Some(fmt_time_hhmm(*ts)),
                    chat: Some(display),
                    username: Some(username.clone()),
                    is_group: Some(is_group),
                    sender: Some(sender_display),
                    content: Some(summary),
                    msg_type: Some(fmt_type(*msg_type)),
                    timestamp: Some(*ts),
                };

                let _ = tx.send(event);
            }

            if !initialized {
                initialized = true;
            }
        }
    });
}

fn mtime_f64(path: &std::path::Path) -> Option<f64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64())
}

fn decompress_or_str(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    if let Ok(dec) = zstd::decode_all(data) {
        if let Ok(s) = String::from_utf8(dec) {
            return s;
        }
    }
    String::from_utf8_lossy(data).into_owned()
}

fn fmt_time_hhmm(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}
