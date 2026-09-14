use super::*;
use crate::daemon::query::encrypted_cache;
use serde_json::json;
use std::fs;

fn golden() -> Value {
    serde_json::from_str(include_str!("golden.json")).unwrap()
}

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
    cached: HashMap<String, PathBuf>,
}

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let db_dir = root.path().join("wxid_self_abcd/db_storage");
    let cache_dir = root.path().join("synthetic-cache");
    fs::create_dir_all(db_dir.join("message")).unwrap();
    fs::create_dir_all(db_dir.join("contact")).unwrap();
    fs::create_dir_all(&cache_dir).unwrap();
    let g = golden();
    let mut mtimes = serde_json::Map::new();
    let mut keys = HashMap::new();
    let mut cached: HashMap<String, PathBuf> = HashMap::new();
    let mut seed = |source: &str| {
        let path = cache_dir.join(format!("{:x}.db", md5::compute(source.as_bytes())));
        keys.insert(source.into(), "11".repeat(32));
        cached.insert(source.into(), path.clone());
        encrypted_cache::sqlite(&path)
    };
    for shard in g["shards"].as_array().unwrap() {
        let conn = seed(shard["source"].as_str().unwrap());
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT)")
            .unwrap();
        for (id, username) in shard["ids"].as_object().unwrap() {
            conn.execute(
                "INSERT INTO Name2Id(rowid,user_name) VALUES(?1,?2)",
                rusqlite::params![id.parse::<i64>().unwrap(), username.as_str().unwrap()],
            )
            .unwrap();
        }
        for (username, rows) in shard["tables"].as_object().unwrap() {
            let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
            conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER)")).unwrap();
            for row in rows.as_array().unwrap() {
                let raw = match row["raw"]["kind"].as_str().unwrap() {
                    "null" => rusqlite::types::Value::Null,
                    "text" => {
                        rusqlite::types::Value::Text(row["raw"]["value"].as_str().unwrap().into())
                    }
                    "bytes" => rusqlite::types::Value::Blob(
                        serde_json::from_value(row["raw"]["value"].clone()).unwrap(),
                    ),
                    other => panic!("unexpected fixture type {other}"),
                };
                conn.execute(
                    &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,?4,?5,?6)"),
                    rusqlite::params![
                        row["local_id"].as_i64(),
                        row["local_type"].as_i64(),
                        row["timestamp"].as_i64(),
                        row["sender_id"].as_i64(),
                        raw,
                        row["compression"].as_i64()
                    ],
                )
                .unwrap();
            }
        }
    }
    let contact = seed("contact/contact.db");
    contact.execute_batch("CREATE TABLE contact(username TEXT,remark TEXT,nick_name TEXT,description TEXT,local_type INTEGER,extra_buffer BLOB); CREATE TABLE contact_label(label_id_,label_name_,sort_order_);").unwrap();
    for c in g["contacts"].as_array().unwrap() {
        contact
            .execute(
                "INSERT INTO contact VALUES(?1,?2,?3,?4,0,NULL)",
                rusqlite::params![
                    c["username"].as_str(),
                    c["remark"].as_str(),
                    c["nick_name"].as_str(),
                    c["description"].as_str()
                ],
            )
            .unwrap();
    }
    drop(contact);
    for (source, path) in &cached {
        let mt = encrypted_cache::seed(path, &db_dir.join(source));
        mtimes.insert(
            source.clone(),
            json!({"db_mt": mt, "wal_mt": 0, "path": path}),
        );
    }
    let mtime = cache_dir.join("_mtimes.json");
    fs::write(&mtime, serde_json::to_vec(&mtimes).unwrap()).unwrap();
    let db = DbCache::with_dirs(db_dir, cache_dir, mtime, keys)
        .await
        .unwrap();
    let mut msg_db_keys: Vec<String> = g["shards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["source"].as_str().unwrap().into())
        .collect();
    msg_db_keys.reverse();
    let names = Names {
        map: serde_json::from_value(g["names"].clone()).unwrap(),
        md5_to_uname: HashMap::new(),
        msg_db_keys,
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    };
    Fixture {
        _root: root,
        db,
        names,
        cached,
    }
}

#[tokio::test]
async fn real_cache_sqlite_adapter_matches_ast_oracle() {
    let f = fixture().await;
    let g = golden();
    let before: HashMap<_, _> = f
        .cached
        .iter()
        .map(|(k, p)| (k, fs::read(p).unwrap()))
        .collect();
    for case in g["cases"].as_array().unwrap() {
        let actual = q_export_delta_username(
            &f.db,
            &f.names,
            case["username"].as_str().unwrap().into(),
            Some(100),
            Some(200),
        )
        .await;
        if case["model"].is_null() {
            assert!(actual.unwrap_err().to_string().contains("no tables"));
            continue;
        }
        let value = actual.unwrap();
        assert_eq!(value, case["model"]);
        let chat: DeltaChat = serde_json::from_value(value).unwrap();
        let window = serde_json::from_value(g["window"].clone()).unwrap();
        let prepared = crate::toolkit::chat_delta::prepare_delta(&chat, &window).unwrap();
        assert_eq!(prepared.result, case["result"]);
        let mut document = prepared.document.unwrap_or(Value::Null);
        // source 是原生导出新增的证据字段，除此之外逐字段对照旧实际文件。
        if let Some(messages) = document.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                message.as_object_mut().unwrap().remove("source");
            }
        }
        assert_eq!(document, case["document"]);
    }
    for (key, path) in &f.cached {
        assert_eq!(fs::read(path).unwrap(), before[key]);
    }
}

#[tokio::test]
async fn start_required_and_window_is_inclusive_without_summary_limit() {
    let f = fixture().await;
    assert!(
        q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), None, None)
            .await
            .is_err()
    );
    let one = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), Some(100))
        .await
        .unwrap();
    assert_eq!(one["messages"].as_array().unwrap().len(), 3);
    assert_eq!(one["messages"][0]["local_id"], 9);
    assert_eq!(one["messages"][1]["local_id"], 2);
    assert_eq!(one["messages"][2]["local_id"], 9);
    let open = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(200), None)
        .await
        .unwrap();
    assert_eq!(open["messages"].as_array().unwrap().len(), 2);
    let empty = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(200), Some(100))
        .await
        .unwrap();
    assert!(empty["messages"].as_array().unwrap().is_empty());
    let username = super::super::resolve_username("合成联系人", &f.names).unwrap();
    let named = q_export_delta_username(&f.db, &f.names, username, Some(100), Some(100))
        .await
        .unwrap();
    assert_eq!(named, one);
}

#[tokio::test]
async fn unknown_or_unreadable_shard_cannot_be_partial_success() {
    let f = fixture().await;
    let unknown = f.db.db_dir().join("message/message_99.db");
    fs::write(&unknown, b"synthetic unknown shard").unwrap();
    let error = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("未知消息分片"));
    fs::remove_file(unknown).unwrap();
    fs::write(
        &f.cached["message/message_1.db"],
        b"synthetic corrupt database",
    )
    .unwrap();
    // Prevent recovery from hiding the intentionally unreadable shard.
    let source = f.db.db_dir().join("message/message_1.db");
    let mut bytes = fs::read(&source).unwrap();
    bytes[4032] ^= 1;
    fs::write(source, bytes).unwrap();
    assert!(
        q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), None)
            .await
            .is_err()
    );
    fs::remove_file(f.db.db_dir().join("message/message_1.db")).unwrap();
    let error = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("无法读取已知消息分片"));
}

#[tokio::test]
async fn group_does_not_require_contact_db_and_single_keeps_warnings() {
    let f = fixture().await;
    let conn = Connection::open(&f.cached["contact/contact.db"]).unwrap();
    conn.execute_batch("DROP TABLE contact_label").unwrap();
    drop(conn);
    let single = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), Some(100))
        .await
        .unwrap();
    assert!(!single["metadata_warnings"].as_array().unwrap().is_empty());
    fs::remove_file(f.db.db_dir().join("contact/contact.db")).unwrap();
    assert!(q_export_delta_username(
        &f.db,
        &f.names,
        "synthetic@chatroom".into(),
        Some(100),
        Some(200)
    )
    .await
    .is_ok());
    assert!(
        q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), Some(200))
            .await
            .is_err()
    );
}

#[test]
fn sqlite_raw_types_and_uid_are_not_replaced_by_rendered_body() {
    let g = golden();
    let chat: DeltaChat = serde_json::from_value(g["cases"][1]["model"].clone()).unwrap();
    let first = &chat.messages[0];
    assert!(matches!(first.raw_content, RawContent::Bytes(_)));
    assert_eq!(first.rendered.as_ref().unwrap(), "压缩正文不是摘要");
    let raw_uid = crate::toolkit::chat_delta::delta_msg_uid(
        &chat.username,
        &first.db_path,
        first.local_id,
        first.timestamp,
        &first.msg_type,
        &first.raw_content,
    );
    let wrong_uid = crate::toolkit::chat_delta::delta_msg_uid(
        &chat.username,
        &first.db_path,
        first.local_id,
        first.timestamp,
        &first.msg_type,
        &RawContent::Text(first.rendered.as_ref().unwrap().as_str().unwrap().into()),
    );
    assert_ne!(raw_uid, wrong_uid);
    assert_eq!(raw_uid, g["cases"][1]["document"]["messages"][0]["msg_uid"]);
    assert!(raw_and_decoded(ValueRef::Integer(7), None).is_err());
    assert!(raw_and_decoded(ValueRef::Text(&[255]), None).is_err());
    assert_eq!(message_type(-4294967295), "type_-4294967295");
    let transfer = chat.messages.iter().find(|m| m.local_id == 6).unwrap();
    assert_eq!(transfer.msg_type, "link_or_file");
    assert_eq!(transfer.extras["type"], "transfer");
    let expected = g["cases"][1]["document"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["local_id"] == 6)
        .unwrap();
    assert_eq!(
        crate::toolkit::chat_delta::delta_msg_uid(
            &chat.username,
            &transfer.db_path,
            transfer.local_id,
            transfer.timestamp,
            "transfer",
            &transfer.raw_content
        ),
        expected["msg_uid"]
    );
}

#[tokio::test]
async fn raw_delta_query_has_no_history_page_limit() {
    let f = fixture().await;
    let mut conn = Connection::open(&f.cached["message/message_0.db"]).unwrap();
    let table = format!("Msg_{:x}", md5::compute(b"wxid_peer"));
    let transaction = conn.transaction().unwrap();
    for id in 10000..10300 {
        transaction
            .execute(
                &format!("INSERT INTO [{table}] VALUES(?1,1,100,2,'synthetic bulk',0)"),
                [id],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(conn);
    let value = q_export_delta_username(&f.db, &f.names, "wxid_peer".into(), Some(100), Some(100))
        .await
        .unwrap();
    assert_eq!(value["messages"].as_array().unwrap().len(), 303);
}
