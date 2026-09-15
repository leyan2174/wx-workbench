//! 引用与附件查询共享的严格消息定位，仅使用显式账号缓存。
use super::{ensure_complete_message_inventory, DbCache, Names};
use crate::{
    adapters::wechat::messages::{RawMessage, Snapshot, SourceFile},
    business::messages::{Error, MessageSelector, SourceKind},
};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(super) const MAX_STORED_BYTES: usize = crate::adapters::wechat::messages::MAX_STORED_BYTES;

pub(super) enum Resolution<T> {
    ChatNotFound,
    AmbiguousChat,
    MessageNotFound,
    AmbiguousMessage,
    Found(T),
}

/// The synchronous callback runs before the read snapshot closes. Return only detached
/// values or existing serializable proofs; references cannot be carried into another read.
pub(super) async fn with_resolved<T, F>(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    read: F,
) -> Result<Resolution<T>>
where
    T: Send + 'static,
    F: FnOnce(&Snapshot, &RawMessage) -> Result<T> + Send + 'static,
{
    with_projection(db, names, chat, local_id, create_time, true, read).await
}

pub(super) async fn with_resolved_metadata<T, F>(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    read: F,
) -> Result<Resolution<T>>
where
    T: Send + 'static,
    F: FnOnce(&Snapshot, &RawMessage) -> Result<T> + Send + 'static,
{
    with_projection(db, names, chat, local_id, create_time, false, read).await
}

async fn with_projection<T, F>(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    with_content: bool,
    read: F,
) -> Result<Resolution<T>>
where
    T: Send + 'static,
    F: FnOnce(&Snapshot, &RawMessage) -> Result<T> + Send + 'static,
{
    let username = match resolve_unique_username(chat, names) {
        ChatResolution::Unique(username) => username,
        ChatResolution::NotFound => return Ok(Resolution::ChatNotFound),
        ChatResolution::Ambiguous => return Ok(Resolution::AmbiguousChat),
    };
    ensure_complete_message_inventory(db, names)?;
    let mut keys = names.msg_db_keys.clone();
    keys.sort();
    keys.dedup();
    let mut shards = Vec::new();
    for key in keys {
        let path = db
            .get(&key)
            .await?
            .with_context(|| format!("unavailable message shard: {key}"))?;
        shards.push((key, path));
    }
    let result = tokio::task::spawn_blocking(move || {
        lookup_with(
            &shards,
            &username,
            local_id,
            create_time,
            with_content,
            read,
        )
    })
    .await?;
    ensure_complete_message_inventory(db, names)?;
    result
}

enum ChatResolution {
    NotFound,
    Unique(String),
    Ambiguous,
}

pub(super) fn username(chat: &str, names: &Names) -> Result<String> {
    match resolve_unique_username(chat, names) {
        ChatResolution::Unique(value) => Ok(value),
        ChatResolution::NotFound => Err(Error::NotFound).context("chat not found"),
        ChatResolution::Ambiguous => Err(Error::Ambiguous).context("ambiguous chat"),
    }
}

fn resolve_unique_username(chat: &str, names: &Names) -> ChatResolution {
    if chat.trim().is_empty() {
        return ChatResolution::NotFound;
    }
    // 精确账号优先；沿用显式 wxid 和群账号入口，以支持联系人表尚未收录的会话。
    if names.map.contains_key(chat) || chat.starts_with("wxid_") || chat.contains("@chatroom") {
        return ChatResolution::Unique(chat.to_owned());
    }
    let lower = chat.to_lowercase();
    for exact in [true, false] {
        let mut candidates = names.map.iter().filter(|(_, display)| {
            let display = display.to_lowercase();
            if exact {
                display == lower
            } else {
                display.contains(&lower)
            }
        });
        if let Some((username, _)) = candidates.next() {
            return if candidates.next().is_some() {
                ChatResolution::Ambiguous
            } else {
                ChatResolution::Unique(username.clone())
            };
        }
    }
    ChatResolution::NotFound
}

fn lookup_with<T, F>(
    shards: &[(String, PathBuf)],
    username: &str,
    local_id: i64,
    create_time: i64,
    with_content: bool,
    read: F,
) -> Result<Resolution<T>>
where
    F: FnOnce(&Snapshot, &RawMessage) -> Result<T>,
{
    let files = shards
        .iter()
        .map(|(key, path)| SourceFile {
            logical_name: key.clone(),
            path: path.clone(),
            kind: SourceKind::Ordinary,
        })
        .collect();
    let snapshot = Snapshot::open(files, [username.to_owned()])?;
    if with_content {
        snapshot.require_target_content(username, SourceKind::Ordinary)?;
    }
    let reference = match snapshot.resolve(
        &MessageSelector {
            username,
            local_id,
            timestamp: (create_time != 0).then_some(create_time),
        },
        SourceKind::Ordinary,
    ) {
        Ok(reference) => reference,
        Err(error) => {
            return match error.downcast_ref::<Error>() {
                Some(Error::NotFound) => Ok(Resolution::MessageNotFound),
                Some(Error::Ambiguous) => Ok(Resolution::AmbiguousMessage),
                _ => Err(error),
            }
        }
    };
    let evidence = if with_content {
        snapshot.read_evidence(reference.evidence())?
    } else {
        snapshot.read_metadata(reference.evidence())?
    };
    snapshot.revalidate(&reference)?;
    let value = read(&snapshot, &evidence)?;
    snapshot.revalidate(&reference)?;
    Ok(Resolution::Found(value))
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    #[test]
    fn metadata_callback_runs_with_live_reference_and_no_body_requirement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("metadata.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        let table = format!("Msg_{:x}", md5::compute("wxid_scope"));
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER); INSERT INTO [{table}] VALUES(7,34,100,42)")).unwrap();
        drop(conn);
        let sources = vec![("message/message_0.db".into(), path)];
        let result = lookup_with(&sources, "wxid_scope", 7, 100, false, |snapshot, raw| {
            snapshot.revalidate(&raw.reference)?;
            assert!(!raw.reference.evidence().is_expired());
            assert_eq!(raw.checked_server_id()?, Some(42));
            Ok(raw.reference.clone())
        })
        .unwrap();
        let Resolution::Found(reference) = result else {
            panic!("expected metadata callback")
        };
        assert!(reference.evidence().is_expired());
        assert!(lookup_with(&sources, "wxid_scope", 7, 100, true, |_, _| Ok(())).is_err());
    }
}
