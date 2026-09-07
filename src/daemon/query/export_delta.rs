//! 显式缓存及联系人上下文的 delta 原始消息查询；不经过 history 阅读摘要。
use super::{current_unknown_shards, msg_table_re, DbCache, Names};
use crate::message::export_content::{extract_with_context, ExportContext};
use crate::toolkit::chat_delta::{ContactMetadata, DeltaChat, DeltaMessage, RawContent};
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// 批量入口按精确 username 查询，禁止再次模糊匹配；任何分片错误均返回 Err。
pub async fn q_export_delta_username(
    db: &DbCache,
    names: &Names,
    username: String,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Value> {
    let start = start.context("delta export requires start_ts")?;
    ensure!(!username.is_empty(), "username 不能为空");
    ensure!(
        current_unknown_shards(db, names).is_empty(),
        "存在未知消息分片，请先更新密钥，不能发布不完整 delta"
    );
    let mut keys = names.msg_db_keys.clone();
    keys.sort();
    keys.dedup();
    let mut shards = Vec::with_capacity(keys.len());
    for key in keys {
        let path = db
            .get(&key)
            .await?
            .with_context(|| format!("无法读取已知消息分片 {key}"))?;
        shards.push((key, path));
    }
    let is_group = username.ends_with("@chatroom");
    let contact_path = if is_group {
        None
    } else {
        Some(
            db.get("contact/contact.db")
                .await?
                .context("无法读取联系人数据库")?,
        )
    };
    let chat = DeltaChat {
        display_name: names.display(&username),
        username,
        is_group,
        contact: ContactMetadata::default(),
        messages: Vec::new(),
        source_error: None,
    };
    let account_dir = db
        .db_dir()
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let me = crate::message::identity::self_username(account_dir, &names.map);
    let names = names.map.clone();
    tokio::task::spawn_blocking(move || {
        let mut chat = read_delta_shards(chat, &me, &names, &shards, start, end)?;
        let mut warnings = Vec::new();
        if let Some(path) = contact_path {
            let metadata = crate::toolkit::contact_metadata::contact_metadata_for_export(
                &path,
                &chat.username,
                false,
            );
            // 与全量导出一致保留旧 schema 回退，同时通过 IPC 交付诊断信息。
            warnings = metadata.diagnostics;
            chat.contact = serde_json::from_value(Value::Object(metadata.fields))?;
        }
        let mut value = serde_json::to_value(chat)?;
        if !warnings.is_empty() {
            value["metadata_warnings"] = serde_json::to_value(warnings)?;
        }
        Ok::<_, anyhow::Error>(value)
    })
    .await?
}

/// 与旧 _msg_type_str 相同：高位只在正值超出 u32 时视为子类型。
fn message_type(local_type: i64) -> String {
    let base = if local_type > u32::MAX as i64 {
        local_type & 0xffff_ffff
    } else {
        local_type
    };
    match base {
        1 => "text",
        3 => "image",
        34 => "voice",
        42 => "contact_card",
        43 => "video",
        47 => "sticker",
        48 => "location",
        49 => "link_or_file",
        50 => "call",
        10000 => "system",
        10002 => "recall",
        _ => return format!("type_{local_type}"),
    }
    .into()
}

fn raw_and_decoded(
    raw: ValueRef<'_>,
    compression: Option<i64>,
) -> Result<(RawContent, Option<String>)> {
    Ok(match raw {
        ValueRef::Null => (RawContent::Null, None),
        ValueRef::Text(bytes) => {
            let text = std::str::from_utf8(bytes)
                .context("SQLite TEXT 不是有效 UTF-8")?
                .to_owned();
            // Python 仅解压 bytes；TEXT 即使标记为 4 也保持原文。
            (RawContent::Text(text.clone()), Some(text))
        }
        ValueRef::Blob(bytes) => {
            let decoded = if compression == Some(4) {
                // 与旧 _decompress_content 一致：损坏压缩帧正文缺省，原始字节仍参与 UID。
                zstd::decode_all(bytes)
                    .ok()
                    .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
            } else {
                Some(String::from_utf8_lossy(bytes).into_owned())
            };
            (RawContent::Bytes(bytes.to_vec()), decoded)
        }
        _ => anyhow::bail!("消息正文必须为 SQLite TEXT、BLOB 或 NULL"),
    })
}

fn read_delta_shards(
    mut chat: DeltaChat,
    me: &str,
    names: &HashMap<String, String>,
    shards: &[(String, PathBuf)],
    start: i64,
    end: Option<i64>,
) -> Result<DeltaChat> {
    let table = format!("Msg_{:x}", md5::compute(chat.username.as_bytes()));
    ensure!(msg_table_re().is_match(&table), "消息表名不合法");
    let context = ExportContext {
        is_group: chat.is_group,
        chat_username: &chat.username,
        chat_display_name: &chat.display_name,
        self_username: me,
        names: Some(names),
    };
    let mut found_table = false;
    for (source, path) in shards {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("打开消息分片失败: {source}"))?;
        let conn = conn.unchecked_transaction()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [&table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        found_table = true;
        // 与消息行共享一个 SQLite 读取快照，防止发送者映射跨版本。
        let mut ids = HashMap::<i64, String>::new();
        let has_ids: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='Name2Id')",
            [],
            |row| row.get(0),
        )?;
        if has_ids {
            let mut statement = conn.prepare("SELECT rowid,user_name FROM Name2Id")?;
            for row in statement.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })? {
                let (id, username) = row?;
                if let Some(username) = username.filter(|s| !s.is_empty()) {
                    ids.insert(id, username);
                }
            }
        }
        // 旧查询只按时间排序；同时间消息不增加 local_id 次排序，也不去重。
        let mut statement = conn.prepare(&format!("SELECT local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content FROM [{table}] WHERE create_time >= ?1 AND (?2 IS NULL OR create_time <= ?2) ORDER BY create_time ASC"))?;
        let mut rows = statement.query(rusqlite::params![start, end])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let kind: i64 = row.get(1)?;
            let timestamp: i64 = row.get(2)?;
            let sender_id: Option<i64> = row.get(3)?;
            let (raw_content, decoded) = raw_and_decoded(row.get_ref(4)?, row.get(5)?)
                .with_context(|| format!("正文适配失败: {source}, local_id={id}"))?;
            let text = decoded.as_deref().unwrap_or("");
            let (prefix, body) = if chat.is_group {
                crate::message::split_group_content(text)
            } else {
                ("", text)
            };
            let mapped = sender_id
                .and_then(|id| ids.get(&id))
                .map(String::as_str)
                .unwrap_or("");
            let sender = crate::message::identity::export_sender(
                mapped,
                prefix,
                chat.is_group,
                &chat.username,
                &chat.display_name,
                me,
                names,
            );
            let extracted = extract_with_context(kind, decoded.as_ref().map(|_| body), &context)?;
            let mut extras = extracted.extras;
            extras.insert("source".into(), Value::String(source.replace('\\', "/")));
            chat.messages.push(DeltaMessage {
                // 缓存实际文件名是哈希；旧 UID 使用 message_N.db，所以必须使用逻辑来源。
                db_path: source.clone(),
                local_id: id,
                timestamp,
                sender,
                msg_type: message_type(kind),
                raw_content,
                rendered: extracted.content.map(Value::String),
                extras,
            });
        }
    }
    ensure!(found_table, "no tables");
    chat.messages.sort_by_key(|message| message.timestamp);
    Ok(chat)
}

#[cfg(test)]
#[path = "../../../tests/fixtures/delta-query/query_tests.rs"]
mod tests;
