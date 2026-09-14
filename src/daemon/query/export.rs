//! 原生单聊导出；格式对齐完成前不替换旧批量入口。
use super::export_directory::{self, ExportShape};
use super::*;
use crate::message::export::{Chat, Message};

pub async fn q_export_chat(db: &DbCache, names: &Names, chat: &str) -> Result<Value> {
    let username = resolve_username(chat, names).context("找不到聊天对象")?;
    q_export_username(db, names, username).await
}

pub async fn q_export_chat_list(db: &DbCache, names: &Names) -> Result<Value> {
    let path = db
        .get("session/session.db")
        .await?
        .context("无法解密 session.db")?;
    let usernames = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = conn.prepare("SELECT username FROM SessionTable")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        anyhow::ensure!(
            rows.iter().all(|u| !u.is_empty()),
            "会话表含空 username，不能静默跳过"
        );
        Ok(rows)
    })
    .await??;
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = usernames
        .into_iter()
        .filter(|u| seen.insert(u.clone()))
        .map(|username| crate::message::export::Target {
            chat: names.display(&username),
            is_group: username.ends_with("@chatroom"),
            username,
        })
        .collect();
    Ok(serde_json::json!({"chats": targets}))
}

// 批量导出的 username 已来自会话表，不能再按显示名模糊匹配到另一联系人。
pub async fn q_export_username(db: &DbCache, names: &Names, username: String) -> Result<Value> {
    q_export_username_with_shape(db, names, username, ExportShape::Compact).await
}

// 两个入口共用分片读取、发送者解析、正文提取及排序；紧凑模式不读取新增列。
pub(super) async fn q_export_username_with_shape(
    db: &DbCache,
    names: &Names,
    username: String,
    shape: ExportShape,
) -> Result<Value> {
    anyhow::ensure!(!username.is_empty(), "username 不能为空");
    anyhow::ensure!(
        current_unknown_shards(db, names).is_empty(),
        "存在未知消息分片，请先更新密钥，不能执行全量导出"
    );
    // 全量导出不能用 MAX(create_time) 发现表：空表和全 NULL 时间戳表也必须保留。
    let mut keys = names.msg_db_keys.clone();
    keys.sort();
    if shape == ExportShape::Directory {
        keys.dedup();
    }
    let mut shards = Vec::new();
    for key in keys {
        let path = db
            .get(&key)
            .await?
            .with_context(|| format!("无法读取已知消息分片 {key}"))?;
        shards.push((key, path));
    }
    let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
    anyhow::ensure!(msg_table_re().is_match(&table), "消息表名不合法");
    let display = names.display(&username);
    let account_dir = db
        .db_dir()
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let me = crate::message::identity::self_username(account_dir, &names.map);
    let contact_path = if username.ends_with("@chatroom") && shape == ExportShape::Compact {
        None
    } else {
        Some(
            db.get("contact/contact.db")
                .await?
                .context("无法读取联系人数据库")?,
        )
    };
    let names = names.map.clone();
    tokio::task::spawn_blocking(move || {
        let is_group = username.ends_with("@chatroom");
        let export_context = crate::message::export_content::ExportContext {
            is_group,
            chat_username: &username,
            chat_display_name: &display,
            self_username: &me,
            names: Some(&names),
        };
        let mut document = Chat { chat: display.clone(), username: username.clone(), exported_at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), is_group, messages: Vec::new() };
        let mut found_table = false;
        let mut sources = Vec::new();
        for (source, path) in shards {
            let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            // Name2Id 和消息行必须来自同一分片读取快照，不能分开读到不同版本。
            let conn = conn.unchecked_transaction()?;
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [&table], |row| row.get(0))?;
            if !exists { continue; }
            found_table = true;
            let source = if shape == ExportShape::Directory {
                source.replace('\\', "/")
            } else { source };
            if shape == ExportShape::Directory {
                sources.push(source.clone());
            }
            let mut ids = HashMap::<i64, String>::new();
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='Name2Id')", [], |row| row.get(0))?;
            if exists {
                let mut stmt = conn.prepare("SELECT rowid,user_name FROM Name2Id")?;
                for row in stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)))? {
                    let (id, name) = row?;
                    if let Some(name) = name.filter(|name| !name.is_empty()) { ids.insert(id, name); }
                }
            }
            let detail_columns = if shape == ExportShape::Directory {
                export_directory::detail_projection(&conn, &table)?
            } else { String::new() };
            let mut statement = conn.prepare(&format!("SELECT local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content{detail_columns} FROM [{table}] ORDER BY create_time ASC,local_id ASC"))?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let kind: i64 = row.get(1)?;
                let timestamp: Option<i64> = row.get(2)?;
                let sender_id: Option<i64> = row.get(3)?;
                let raw = row.get_ref(4)?;
                let was_null = matches!(raw, rusqlite::types::ValueRef::Null);
                let bytes = match raw {
                    rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => bytes,
                    rusqlite::types::ValueRef::Null => &[],
                    _ => anyhow::bail!("消息正文类型异常，local_id={id}"),
                };
                let compression: Option<i64> = row.get(5)?;
                let text = if shape == ExportShape::Directory && was_null { String::new() }
                    else if compression == Some(4) { String::from_utf8(zstd::decode_all(bytes)?)? }
                    else { String::from_utf8(bytes.to_vec())? };
                let prefix = if is_group { crate::message::split_group_content(&text).0 } else { "" };
                let mapped = sender_id.and_then(|id| ids.get(&id)).map(String::as_str).unwrap_or("");
                let sender = crate::message::identity::export_sender(mapped, prefix, is_group, &username, &display, &me, &names);
                let body = if is_group { crate::message::split_group_content(&text).1 } else { &text };
                let extracted = crate::message::export_content::extract_with_context(
                    kind, (!was_null).then_some(body), &export_context,
                )?;
                let mut extras = extracted.extras;
                extras.insert("source".into(), Value::String(source.clone()));
                if shape == ExportShape::Directory {
                    export_directory::append_details(
                        &mut extras, row, kind, mapped, prefix, is_group, &username,
                        (!was_null).then_some(text.as_str()),
                    ).with_context(|| format!("目录导出消息字段无效: {source}, local_id={id}"))?;
                }
                document.messages.push(Message::new(id, kind, timestamp, sender, extracted.content, extras)?);
            }
        }
        anyhow::ensure!(found_table, "找不到聊天消息表");
        document.sort_chronologically();
        let mut value = serde_json::to_value(document)?;
        if shape == ExportShape::Directory {
            sources.sort();
            sources.dedup();
            value["sources"] = serde_json::to_value(sources)?;
            value["contact_alias"] = Value::Null;
        }
        if let Some(path) = contact_path {
            let metadata = crate::toolkit::contact_metadata::contact_metadata_for_export(&path, &username, is_group);
            value.as_object_mut().unwrap().extend(metadata.fields);
            if !metadata.diagnostics.is_empty() {
                value["metadata_warnings"] = serde_json::to_value(metadata.diagnostics)?;
            }
            if shape == ExportShape::Directory {
                value["contact_alias"] = export_directory::contact_alias(&path, &username)?;
            }
        }
        Ok::<_, anyhow::Error>(value)
    }).await?
}
