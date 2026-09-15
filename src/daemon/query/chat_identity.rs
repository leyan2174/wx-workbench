//! Resolve only identities evidenced by the selected account, before display-name matching.
use super::{ensure_complete_message_inventory, strict_message, DbCache, Names};
use crate::adapters::wechat::messages::{session_identity, sources, Snapshot, SourceFile};
use crate::business::messages::{Conversation, Error, SourceKind};
use anyhow::{Context, Result};
use std::collections::HashSet;

pub(super) async fn session_usernames(db: &DbCache) -> Result<Vec<String>> {
    let Some(path) = db.get(sources::sessions().cache_key()).await? else {
        return Ok(Vec::new());
    };
    tokio::task::spawn_blocking(move || session_identity::usernames(&path)).await?
}

pub(super) async fn message_usernames(
    db: &DbCache,
    names: &Names,
    sessions: Vec<String>,
) -> Result<HashSet<String>> {
    ensure_complete_message_inventory(db, names)?;
    let mut keys = names.msg_db_keys.clone();
    keys.sort();
    keys.dedup();
    let mut files = Vec::new();
    for key in keys {
        let path = db
            .get(&key)
            .await?
            .context(Error::Unavailable)
            .context("message identity source unavailable")?;
        files.push(SourceFile {
            logical_name: key,
            path,
            kind: SourceKind::Ordinary,
        });
    }
    let identities: Vec<_> = names.map.keys().cloned().chain(sessions).collect();
    let result = tokio::task::spawn_blocking(move || -> Result<HashSet<String>> {
        let snapshot = Snapshot::open(files, identities)?;
        Ok(snapshot
            .streams()
            .iter()
            .filter_map(|entry| match &entry.conversation {
                Conversation::Known(username) => Some(username.clone()),
                Conversation::Unmapped(_) => None,
            })
            .collect())
    })
    .await?;
    ensure_complete_message_inventory(db, names)?;
    result
}

pub(super) async fn exact(db: &DbCache, names: &Names, chat: &str) -> Result<bool> {
    if chat.trim().is_empty() {
        return Ok(false);
    }
    if names.map.contains_key(chat) {
        return Ok(true);
    }
    let sessions = session_usernames(db).await?;
    if sessions.iter().any(|username| username == chat) {
        return Ok(true);
    }
    if names.msg_db_keys.is_empty() {
        return Ok(false);
    }
    Ok(message_usernames(db, names, sessions).await?.contains(chat))
}

pub async fn resolve(db: &DbCache, names: &Names, chat: &str) -> Result<String> {
    match exact(db, names, chat).await {
        Ok(true) => Ok(chat.to_owned()),
        Ok(false) => strict_message::username(chat, names),
        Err(error) => {
            // Ambiguity is still a safe refusal, but an unreadable catalog must never
            // redirect a possibly exact identity to a different contact's display name.
            if let Err(display_error) = strict_message::username(chat, names) {
                if matches!(
                    display_error.downcast_ref::<Error>(),
                    Some(Error::Ambiguous)
                ) {
                    return Err(display_error);
                }
            }
            Err(error)
        }
    }
}

pub(super) async fn require_exact(db: &DbCache, names: &Names, chat: &str) -> Result<()> {
    anyhow::ensure!(exact(db, names, chat).await?, Error::NotFound);
    Ok(())
}

pub(super) fn is_folded(username: &str) -> bool {
    matches!(username, "brandsessionholder" | "@placeholder_foldgroup")
}

pub(super) fn mark_exportability(value: &mut serde_json::Value, has_message_table: Option<bool>) {
    let folded = value["username"].as_str().is_some_and(is_folded);
    if !folded {
        value["exportable"] = serde_json::json!(true);
        return;
    }
    value["exportable"] = serde_json::json!(has_message_table);
    match has_message_table {
        Some(false) => {
            value["skip_reason"] = serde_json::json!("system_placeholder_without_message_table")
        }
        None => value["skip_reason"] = serde_json::json!("message_source_unavailable"),
        Some(true) => {}
    }
}
