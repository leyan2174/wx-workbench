//! 目录专用消息表目录；Name2Id 和联系人仅补身份，SessionTable 不决定导出范围。
use super::{ensure_complete_message_inventory, DbCache, Names};
use crate::message::export::{Chat, Message, Target};
use anyhow::{ensure, Context, Result};
#[cfg(test)]
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Debug, Serialize)]
pub(super) struct Entry {
    #[serde(flatten)]
    pub target: Target,
    pub table_name: String,
    pub identity_status: &'static str,
    pub sources: Vec<String>,
}

pub(super) struct Catalog {
    pub entries: Vec<Entry>,
    shards: Vec<(String, PathBuf)>,
}

pub(super) async fn load(db: &DbCache, names: &Names) -> Result<Catalog> {
    ensure_complete_message_inventory(db, names)?;
    let keys: BTreeSet<_> = names.msg_db_keys.iter().cloned().collect();
    let mut shards = Vec::new();
    for key in keys {
        // DbCache 的密钥映射按原键查找；只有输出的逻辑 source 才统一分隔符。
        let path = db
            .get(&key)
            .await?
            .with_context(|| format!("无法读取已知消息分片 {key}"))?;
        shards.push((key, path));
    }
    let snapshot_names = names.clone();
    let result = tokio::task::spawn_blocking(move || {
        let entries = read(&shards, &snapshot_names)?;
        Ok::<_, anyhow::Error>(Catalog { entries, shards })
    })
    .await?;
    ensure_complete_message_inventory(db, names)?;
    result
}

fn read(shards: &[(String, PathBuf)], names: &Names) -> Result<Vec<Entry>> {
    use crate::adapters::wechat::messages::{catalog, Snapshot, SourceFile};
    use crate::business::messages::{Conversation, SourceKind};
    if shards.is_empty() {
        return Ok(Vec::new());
    }
    let files = shards
        .iter()
        .map(|(logical_name, path)| SourceFile {
            logical_name: logical_name.clone(),
            path: path.clone(),
            kind: SourceKind::Ordinary,
        })
        .collect();
    let snapshot = Snapshot::open(files, names.map.keys().cloned())?;
    let rows = catalog::read(&snapshot, SourceKind::Ordinary)?;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for row in rows {
        let (username, mapped) = match row.conversation {
            Conversation::Known(username) => (username, true),
            Conversation::Unmapped(reference) => {
                let hash =
                    crate::adapters::wechat::messages::read::diagnostics::legacy_unmapped_key(
                        &reference,
                    );
                let username = format!("unknown_{hash}");
                ensure!(
                    !names.map.contains_key(&username) && !snapshot.has_sender_username(&username),
                    "目录占位身份与已知 username 冲突，拒绝错误关联"
                );
                (username, false)
            }
        };
        ensure!(
            seen.insert(username.clone()),
            "目录占位身份与真实 username 冲突，拒绝合并不同消息表"
        );
        entries.push(Entry {
            target: Target {
                chat: if mapped {
                    names.display(&username)
                } else {
                    username.clone()
                },
                is_group: mapped && username.ends_with("@chatroom"),
                username,
            },
            table_name: row.table_name,
            identity_status: if mapped { "mapped" } else { "unmapped" },
            sources: row.sources,
        });
    }
    entries.sort_by(|a, b| a.target.username.cmp(&b.target.username));
    Ok(entries)
}

pub(super) async fn export_unmapped(
    db: &DbCache,
    names: &Names,
    catalog: Catalog,
    username: String,
) -> Result<Value> {
    let entry = catalog
        .entries
        .into_iter()
        .find(|entry| entry.target.username == username)
        .context("未映射消息表已不在目录中")?;
    ensure!(
        entry.identity_status == "unmapped",
        "消息表已有真实 username，不能使用占位身份"
    );
    let account_dir = db
        .db_dir()
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let me = crate::message::identity::self_username(account_dir, &names.map);
    let names = names.clone();
    let shards = catalog.shards;
    tokio::task::spawn_blocking(move || read_unmapped(&shards, &names, &entry, &me)).await?
}

// 只有无法求得 username 的表使用此分支；仍复用既有消息解析、详细字段和排序契约。
fn read_unmapped(
    shards: &[(String, PathBuf)],
    names: &Names,
    entry: &Entry,
    me: &str,
) -> Result<Value> {
    let table = &entry.table_name;
    let hash = crate::adapters::wechat::messages::read::layout::canonical_table_hash(table)
        .context("未映射表身份无效")?;
    ensure!(
        entry.identity_status == "unmapped" && entry.target.username == format!("unknown_{hash}"),
        "未映射表身份无效"
    );
    let target = &entry.target;
    let context = crate::adapters::wechat::messages::export_content::ExportContext {
        is_group: false,
        chat_username: &target.username,
        chat_display_name: &target.chat,
        self_username: me,
        names: Some(&names.map),
    };
    let mut document = Chat {
        chat: target.chat.clone(),
        username: target.username.clone(),
        exported_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        is_group: false,
        messages: Vec::new(),
    };
    use crate::adapters::wechat::messages::{read::export::ExportProfile, Snapshot, SourceFile};
    use crate::business::messages::{Conversation, Filter, SourceKind};
    let files = shards
        .iter()
        .map(|(logical_name, path)| SourceFile {
            logical_name: logical_name.clone(),
            path: path.clone(),
            kind: SourceKind::Ordinary,
        })
        .collect();
    let snapshot = Snapshot::open(files, names.map.keys().cloned())?;
    let mut sources = BTreeSet::new();
    for (stream, item) in snapshot.streams().iter().enumerate() {
        if !item.table_name().eq_ignore_ascii_case(table) {
            continue;
        }
        ensure!(
            matches!(&item.conversation, Conversation::Unmapped(_)),
            "读取期间未映射表获得了 username，请重新获取目录"
        );
        let source = snapshot.source_name(stream)?.replace('\\', "/");
        sources.insert(source.clone());
        snapshot.visit_export(
            stream,
            &Filter::default(),
            ExportProfile::Directory,
            |row| {
                let mapped = row.mapped_sender.as_deref().unwrap_or("");
                let sender = crate::message::identity::export_sender(
                    mapped,
                    "",
                    false,
                    &target.username,
                    &target.chat,
                    me,
                    &names.map,
                );
                let extracted =
                    crate::adapters::wechat::messages::export_content::extract_with_context(
                        row.local_type,
                        row.decoded.as_deref(),
                        &context,
                    )?;
                let mut extras = extracted.extras;
                extras.insert("source".into(), Value::String(source.clone()));
                extras.insert("table_name".into(), Value::String(table.clone()));
                row.append_details(&mut extras, "", false, &target.username)?;
                document.messages.push(Message::new(
                    row.local_id,
                    row.local_type,
                    row.timestamp,
                    sender,
                    extracted.content,
                    extras,
                )?);
                Ok(())
            },
        )?;
    }
    ensure!(!sources.is_empty(), "找不到目录中的未映射消息表");
    document.sort_chronologically();
    let mut value = serde_json::to_value(document)?;
    value["sources"] = serde_json::to_value(sources)?;
    value["contact_alias"] = Value::Null;
    value["table_name"] = table.clone().into();
    value["identity_status"] = "unmapped".into();
    value["group_status"] = "unknown".into();
    value["metadata_warnings"] = json!(["消息表缺少 username 映射，保留完整表哈希；占位 ID 不是已确认联系人，群聊身份与媒体归属未确认"]);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn names() -> Names {
        Names {
            map: HashMap::new(),
            msg_db_keys: Vec::new(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        }
    }

    fn table(username: &str) -> String {
        format!("Msg_{:x}", md5::compute(username.as_bytes()))
    }

    fn create_table(conn: &Connection, table: &str) {
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,
            real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER,server_id,sort_seq INTEGER,status INTEGER);
            INSERT INTO [{table}] VALUES(7,1,123,1,'orphan history',0,'0009223372036854775808',NULL,NULL);")).unwrap();
    }

    #[test]
    fn directory_catalog_keeps_unmapped_table_without_session_or_name2id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message_0.db");
        let conn = Connection::open(&path).unwrap();
        let table = table("lost");
        create_table(&conn, &table);
        drop(conn);
        let shards = vec![("message\\message_0.db".into(), path.clone())];
        let before = std::fs::read(&path).unwrap();
        let entries = read(&shards, &names()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].target.username,
            format!("unknown_{}", &table[4..])
        );
        assert_eq!(entries[0].identity_status, "unmapped");
        assert_eq!(entries[0].target.chat, entries[0].target.username);
        let value = read_unmapped(&shards, &names(), &entries[0], "").unwrap();
        assert_eq!(value["messages"].as_array().unwrap().len(), 1);
        let row = &value["messages"][0];
        assert_eq!(row["raw_content"], "orphan history");
        assert_eq!(row["source"], "message/message_0.db");
        assert_eq!(row["table_name"], table);
        assert_eq!(row["server_id"], "0009223372036854775808");
        assert!(
            row["sort_seq"].is_null()
                && row["status"].is_null()
                && row["sender_username"].is_null()
        );
        assert_eq!(value["group_status"], "unknown");
        assert_eq!(std::fs::read(path).unwrap(), before);
        let mut conflicting_names = names();
        conflicting_names.map.insert(
            entries[0].target.username.clone(),
            "Different contact".into(),
        );
        assert!(read(&shards, &conflicting_names).is_err());
    }

    #[test]
    fn directory_catalog_deduplicates_tables_and_recovers_orphan_name2id_globally() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("message_0.db");
        let second = dir.path().join("message_1.db");
        let conn = Connection::open(&first).unwrap();
        create_table(&conn, &table("normal"));
        create_table(&conn, &table("orphan"));
        drop(conn);
        let conn = Connection::open(&second).unwrap();
        create_table(&conn, &table("normal"));
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('normal'),('orphan'),('normal');").unwrap();
        drop(conn);
        let conn = Connection::open(dir.path().join("session.db")).unwrap();
        conn.execute_batch("CREATE TABLE SessionTable(username TEXT); INSERT INTO SessionTable VALUES('normal'),('normal'),('session_only');").unwrap();
        drop(conn);
        let mut names = names();
        names.map.insert("normal".into(), "Normal display".into());
        let shards = vec![
            ("message/message_0.db".into(), first),
            ("message/message_1.db".into(), second),
        ];
        let entries = read(&shards, &names).unwrap();
        assert_eq!(entries.len(), 2);
        let normal = entries
            .iter()
            .find(|entry| entry.target.username == "normal")
            .unwrap();
        assert_eq!(normal.target.chat, "Normal display");
        assert_eq!(
            normal.sources,
            ["message/message_0.db", "message/message_1.db"]
        );
        let orphan = entries
            .iter()
            .find(|entry| entry.target.username == "orphan")
            .unwrap();
        assert_eq!(orphan.identity_status, "mapped");
        assert_eq!(orphan.sources, ["message/message_0.db"]);
        assert!(entries
            .iter()
            .all(|entry| entry.target.username != "session_only"));
    }

    #[test]
    fn directory_catalog_does_not_merge_unmapped_tables_with_equal_legacy_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message_0.db");
        let conn = Connection::open(&path).unwrap();
        create_table(&conn, "Msg_12345678000000000000000000000000");
        create_table(&conn, "Msg_12345678111111111111111111111111");
        drop(conn);
        let entries = read(&[("message/message_0.db".into(), path)], &names()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].target.username, entries[1].target.username);
    }

    #[test]
    fn directory_unmapped_export_preserves_compressed_rows_and_duplicate_ids_across_shards() {
        let dir = tempfile::tempdir().unwrap();
        let table = table("lost");
        let mut shards = Vec::new();
        for index in 0..2 {
            let path = dir.path().join(format!("message_{index}.db"));
            let conn = Connection::open(&path).unwrap();
            create_table(&conn, &table);
            conn.execute_batch(
                "CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('sender');",
            )
            .unwrap();
            if index == 1 {
                let compressed = zstd::encode_all(&b"compressed original"[..], 1).unwrap();
                conn.execute(&format!("UPDATE [{table}] SET message_content=?1,WCDB_CT_message_content=4,create_time=124"),
                    [compressed]).unwrap();
            }
            drop(conn);
            shards.push((format!("message/message_{index}.db"), path));
        }
        let entries = read(&shards, &names()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sources.len(), 2);
        let value = read_unmapped(&shards, &names(), &entries[0], "").unwrap();
        let rows = value["messages"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["local_id"], rows[1]["local_id"]);
        assert_ne!(rows[0]["source"], rows[1]["source"]);
        assert_eq!(rows[1]["raw_content"], "compressed original");
        assert_eq!(rows[1]["sender_username"], "sender");
        assert_eq!(rows[1]["sender"], "sender");
    }

    #[test]
    fn directory_catalog_rejects_invalid_tables_and_unmapped_bad_field_types() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message_0.db");
        let conn = Connection::open(&path).unwrap();
        let table = table("lost");
        create_table(&conn, &table);
        conn.execute_batch(&format!("UPDATE [{table}] SET server_id=1.5;"))
            .unwrap();
        let shards = vec![("message/message_0.db".into(), path)];
        let entries = read(&shards, &names()).unwrap();
        assert!(read_unmapped(&shards, &names(), &entries[0], "").is_err());
        conn.execute_batch("CREATE TABLE Msg_not_a_hash(x INTEGER)")
            .unwrap();
        assert!(read(&shards, &names()).is_err());
    }
}
