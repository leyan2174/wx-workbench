//! 显式缓存及联系人上下文的 delta 原始消息查询；不经过 history 阅读摘要。
use super::{current_unknown_shards, ensure_complete_message_inventory, DbCache, Names};
use crate::message::export_content::{extract_with_context, ExportContext};
use crate::toolkit::chat_delta::{ContactMetadata, DeltaChat, DeltaMessage, RawContent};
use anyhow::{ensure, Context, Result};
#[cfg(test)]
use rusqlite::{types::ValueRef, Connection};
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
            db.get(crate::adapters::wechat::messages::sources::contacts().cache_key())
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
    let account_names = names;
    ensure_complete_message_inventory(db, account_names)?;
    let names = names.map.clone();
    let result = tokio::task::spawn_blocking(move || {
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
    .await?;
    ensure_complete_message_inventory(db, account_names)?;
    result
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

fn delta_raw(content: crate::adapters::wechat::messages::StoredContent) -> Result<RawContent> {
    use crate::adapters::wechat::messages::StoredContent;
    Ok(match content {
        StoredContent::Null => RawContent::Null,
        StoredContent::Text(bytes) => RawContent::Text(String::from_utf8(bytes)?),
        StoredContent::Blob(bytes) => RawContent::Bytes(bytes),
        _ => anyhow::bail!("raw export content unavailable"),
    })
}

#[cfg(test)]
fn raw_and_decoded(
    raw: ValueRef<'_>,
    compression: Option<i64>,
) -> Result<(RawContent, Option<String>)> {
    use crate::adapters::wechat::messages::read::export::{decode_value, ExportProfile};
    let (raw, decoded) = decode_value(raw, compression, ExportProfile::Delta)?;
    Ok((delta_raw(raw)?, decoded))
}

fn read_delta_shards(
    mut chat: DeltaChat,
    me: &str,
    names: &HashMap<String, String>,
    shards: &[(String, PathBuf)],
    start: i64,
    end: Option<i64>,
) -> Result<DeltaChat> {
    let context = ExportContext {
        is_group: chat.is_group,
        chat_username: &chat.username,
        chat_display_name: &chat.display_name,
        self_username: me,
        names: Some(names),
    };
    use crate::adapters::wechat::messages::{read::export::ExportProfile, Snapshot, SourceFile};
    use crate::business::messages::{Filter, SourceKind};
    let files = shards
        .iter()
        .map(|(logical_name, path)| SourceFile {
            logical_name: logical_name.clone(),
            path: path.clone(),
            kind: SourceKind::Ordinary,
        })
        .collect();
    let snapshot = Snapshot::open(
        files,
        names
            .keys()
            .cloned()
            .chain(std::iter::once(chat.username.clone())),
    )?;
    let streams = snapshot.streams_for(&chat.username, SourceKind::Ordinary);
    ensure!(!streams.is_empty(), "no tables");
    let filter = Filter {
        since: Some(start),
        until: end,
        kinds: Vec::new(),
    };
    for stream in streams {
        let source = snapshot.source_name(stream)?.to_owned();
        snapshot.visit_export(stream, &filter, ExportProfile::Delta, |row| {
            let text = row.decoded.as_deref().unwrap_or("");
            let (prefix, body) = if chat.is_group {
                crate::message::split_group_content(text)
            } else {
                ("", text)
            };
            let mapped = row.mapped_sender.as_deref().unwrap_or("");
            let sender = crate::message::identity::export_sender(
                mapped,
                prefix,
                chat.is_group,
                &chat.username,
                &chat.display_name,
                me,
                names,
            );
            let extracted =
                extract_with_context(row.local_type, row.decoded.as_ref().map(|_| body), &context)?;
            let mut extras = extracted.extras;
            extras.insert("source".into(), Value::String(source.replace('\\', "/")));
            chat.messages.push(DeltaMessage {
                db_path: source.clone(),
                local_id: row.local_id,
                timestamp: row.timestamp.context("delta timestamp unavailable")?,
                sender,
                msg_type: message_type(row.local_type),
                raw_content: delta_raw(row.content)?,
                rendered: extracted.content.map(Value::String),
                extras,
            });
            Ok(())
        })?;
    }
    chat.messages.sort_by_key(|message| message.timestamp);
    Ok(chat)
}

#[cfg(test)]
#[path = "../../../tests/fixtures/delta-query/query_tests.rs"]
mod tests;
