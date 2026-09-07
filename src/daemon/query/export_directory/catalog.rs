//! 目录专用消息表目录；Name2Id 和联系人仅补身份，SessionTable 不决定导出范围。
use super::{append_details, detail_projection, ensure_complete_message_inventory, DbCache, Names};
use crate::message::export::{Chat, Message, Target};
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

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

fn senders(conn: &Connection) -> Result<BTreeMap<i64, String>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='Name2Id')",
        [],
        |row| row.get(0),
    )?;
    let mut result = BTreeMap::new();
    if exists {
        let mut statement = conn.prepare("SELECT rowid,user_name FROM Name2Id")?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })? {
            let (id, username) = row?;
            if let Some(username) = username.filter(|username| !username.is_empty()) {
                result.insert(id, username);
            }
        }
    }
    Ok(result)
}

fn add_username(lookup: &mut BTreeMap<String, String>, username: &str) -> Result<()> {
    if !username.is_empty() {
        let hash = format!("{:x}", md5::compute(username.as_bytes()));
        if let Some(previous) = lookup.insert(hash, username.into()) {
            ensure!(
                previous == username,
                "消息表哈希对应多个 username，拒绝任取身份"
            );
        }
    }
    Ok(())
}

fn read(shards: &[(String, PathBuf)], names: &Names) -> Result<Vec<Entry>> {
    let mut lookup = BTreeMap::new();
    for username in names.map.keys() {
        add_username(&mut lookup, username)?;
    }
    let mut tables: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source, path) in shards {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let snapshot = conn.unchecked_transaction()?;
        for username in senders(&snapshot)?.values() {
            add_username(&mut lookup, username)?;
        }
        let mut statement = snapshot.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'Msg_*' ORDER BY name",
        )?;
        for table in statement.query_map([], |row| row.get::<_, String>(0))? {
            let table = table?;
            ensure!(
                super::super::msg_table_re().is_match(&table),
                "消息分片含不支持的 Msg_ 表名：{source}"
            );
            tables
                .entry(table)
                .or_default()
                .insert(source.replace('\\', "/"));
        }
    }
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for (table_name, sources) in tables {
        let hash = &table_name[4..];
        let mapped = lookup.get(hash);
        let username = mapped.cloned().unwrap_or_else(|| format!("unknown_{hash}"));
        if mapped.is_none() {
            ensure!(
                !lookup.contains_key(&format!("{:x}", md5::compute(username.as_bytes()))),
                "目录占位身份与已知 username 冲突，拒绝错误关联"
            );
        }
        ensure!(
            seen.insert(username.clone()),
            "目录占位身份与真实 username 冲突，拒绝合并不同消息表"
        );
        let target = Target {
            chat: if mapped.is_some() {
                names.display(&username)
            } else {
                username.clone()
            },
            is_group: mapped.is_some() && username.ends_with("@chatroom"),
            username,
        };
        entries.push(Entry {
            target,
            table_name,
            identity_status: if mapped.is_some() {
                "mapped"
            } else {
                "unmapped"
            },
            sources: sources.into_iter().collect(),
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
    ensure!(
        super::super::msg_table_re().is_match(table)
            && entry.identity_status == "unmapped"
            && entry.target.username == format!("unknown_{}", &table[4..]),
        "未映射表身份无效"
    );
    let target = &entry.target;
    let context = crate::message::export_content::ExportContext {
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
    let mut sources = BTreeSet::new();
    for (source, path) in shards {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let snapshot = conn.unchecked_transaction()?;
        let ids = senders(&snapshot)?;
        ensure!(
            !ids.values()
                .any(|username| format!("Msg_{:x}", md5::compute(username.as_bytes())) == *table),
            "读取期间未映射表获得了 username，请重新获取目录"
        );
        let exists: bool = snapshot.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        let source = source.replace('\\', "/");
        sources.insert(source.clone());
        let details = detail_projection(&snapshot, table)?;
        let mut statement = snapshot.prepare(&format!(
            "SELECT local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content{details} FROM [{table}] ORDER BY create_time ASC,local_id ASC"))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let kind: i64 = row.get(1)?;
            let timestamp: Option<i64> = row.get(2)?;
            let sender_id: Option<i64> = row.get(3)?;
            let raw = row.get_ref(4)?;
            let was_null = matches!(raw, ValueRef::Null);
            let bytes = match raw {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
                ValueRef::Null => &[],
                _ => anyhow::bail!("消息正文类型异常，local_id={id}"),
            };
            let compression: Option<i64> = row.get(5)?;
            let text = if was_null {
                String::new()
            } else if compression == Some(4) {
                String::from_utf8(zstd::decode_all(bytes)?)?
            } else {
                String::from_utf8(bytes.to_vec())?
            };
            let mapped = sender_id
                .and_then(|id| ids.get(&id))
                .map(String::as_str)
                .unwrap_or("");
            let sender = crate::message::identity::export_sender(
                mapped,
                "",
                false,
                &target.username,
                &target.chat,
                me,
                &names.map,
            );
            let extracted = crate::message::export_content::extract_with_context(
                kind,
                (!was_null).then_some(text.as_str()),
                &context,
            )?;
            let mut extras = extracted.extras;
            extras.insert("source".into(), Value::String(source.clone()));
            extras.insert("table_name".into(), Value::String(table.clone()));
            append_details(
                &mut extras,
                row,
                kind,
                mapped,
                "",
                false,
                &target.username,
                (!was_null).then_some(text.as_str()),
            )
            .with_context(|| format!("目录导出消息字段无效: {source}, local_id={id}"))?;
            document.messages.push(Message::new(
                id,
                kind,
                timestamp,
                sender,
                extracted.content,
                extras,
            )?);
        }
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
            md5_to_uname: HashMap::new(),
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
