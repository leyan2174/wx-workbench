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
        .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
        .await?
        .context("无法解密 session.db")?;
    let usernames = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        crate::adapters::wechat::messages::sessions::usernames(&path)
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
            db.get(crate::adapters::wechat::messages::sources::contacts().cache_key())
                .await?
                .context("无法读取联系人数据库")?,
        )
    };
    let account_names = names;
    ensure_complete_message_inventory(db, account_names)?;
    let names = names.map.clone();
    let result = tokio::task::spawn_blocking(move || {
        let is_group = username.ends_with("@chatroom");
        let export_context = crate::message::export_content::ExportContext {
            is_group,
            chat_username: &username,
            chat_display_name: &display,
            self_username: &me,
            names: Some(&names),
        };
        let mut document = Chat {
            chat: display.clone(),
            username: username.clone(),
            exported_at: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            is_group,
            messages: Vec::new(),
        };
        use crate::adapters::wechat::messages::{
            read::export::ExportProfile, Snapshot, SourceFile,
        };
        use crate::business::messages::{Filter, SourceKind};
        let files = shards
            .into_iter()
            .map(|(logical_name, path)| SourceFile {
                logical_name,
                path,
                kind: SourceKind::Ordinary,
            })
            .collect();
        let snapshot = Snapshot::open(
            files,
            names
                .keys()
                .cloned()
                .chain(std::iter::once(username.clone())),
        )?;
        let streams = snapshot.streams_for(&username, SourceKind::Ordinary);
        anyhow::ensure!(!streams.is_empty(), "找不到聊天消息表");
        let mut sources = Vec::new();
        for stream in streams {
            let source = snapshot.source_name(stream)?.to_owned();
            let source = if shape == ExportShape::Directory {
                source.replace('\\', "/")
            } else {
                source
            };
            if shape == ExportShape::Directory {
                sources.push(source.clone());
            }
            let profile = if shape == ExportShape::Directory {
                ExportProfile::Directory
            } else {
                ExportProfile::Compact
            };
            snapshot.visit_export(stream, &Filter::default(), profile, |row| {
                let id = row.local_id;
                let kind = row.local_type;
                let text = row.decoded.as_deref().unwrap_or("");
                let prefix = if is_group {
                    crate::message::split_group_content(text).0
                } else {
                    ""
                };
                let mapped = row.mapped_sender.as_deref().unwrap_or("");
                let sender = crate::message::identity::export_sender(
                    mapped, prefix, is_group, &username, &display, &me, &names,
                );
                let body = if is_group {
                    crate::message::split_group_content(text).1
                } else {
                    text
                };
                let extracted = crate::message::export_content::extract_with_context(
                    kind,
                    (!matches!(
                        row.content,
                        crate::adapters::wechat::messages::StoredContent::Null
                    ))
                    .then_some(body),
                    &export_context,
                )?;
                let mut extras = extracted.extras;
                extras.insert("source".into(), Value::String(source.clone()));
                if shape == ExportShape::Directory {
                    row.append_details(&mut extras, prefix, is_group, &username)?;
                }
                document.messages.push(Message::new(
                    id,
                    kind,
                    row.timestamp,
                    sender,
                    extracted.content,
                    extras,
                )?);
                Ok(())
            })?;
        }
        document.sort_chronologically();
        let mut value = serde_json::to_value(document)?;
        if shape == ExportShape::Directory {
            sources.sort();
            sources.dedup();
            value["sources"] = serde_json::to_value(sources)?;
            value["contact_alias"] = Value::Null;
        }
        if let Some(path) = contact_path {
            let metadata = crate::toolkit::contact_metadata::contact_metadata_for_export(
                &path, &username, is_group,
            );
            value.as_object_mut().unwrap().extend(metadata.fields);
            if !metadata.diagnostics.is_empty() {
                value["metadata_warnings"] = serde_json::to_value(metadata.diagnostics)?;
            }
            if shape == ExportShape::Directory {
                value["contact_alias"] = export_directory::contact_alias(&path, &username)?;
            }
        }
        Ok::<_, anyhow::Error>(value)
    })
    .await?;
    ensure_complete_message_inventory(db, account_names)?;
    result
}
