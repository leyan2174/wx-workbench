//! 真实查询的消息身份回归；全部数据库都在独立临时目录内合成。
use super::{q_history, q_new_messages, DbCache, Names};
use rusqlite::params;
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path};

#[path = "../../../tests/fixtures/mcp-readonly-runtime/encrypted_sqlite.rs"]
mod encrypted_sqlite;

const PEER: &str = "wxid_source_fixture";
const BASE: i64 = 1_700_000_000;
const RAW_SOURCE_0: &str = "message\\message_0.db";
const SOURCE_0: &str = "message/message_0.db";
const SOURCE_1: &str = "message/message_1.db";

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
}

async fn fixture() -> Fixture {
    fixture_with_extra(None).await
}

async fn fixture_with_extra(extra: Option<(i64, i64, i64, &str)>) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("synthetic-account/db_storage");
    let cache = root.path().join("cache");
    fs::create_dir_all(storage.join("message")).unwrap();
    fs::create_dir_all(storage.join("session")).unwrap();
    let table = format!("Msg_{:x}", md5::compute(PEER.as_bytes()));
    let shards = [
        (RAW_SOURCE_0, [(17, 100, 1), (7, 200, 3), (44, 500, 1)]),
        (SOURCE_1, [(7, 200, 1), (29, 300, 3), (91, 400, 34)]),
    ];
    let mut keys = HashMap::new();
    for (index, (source, rows)) in shards.into_iter().enumerate() {
        let plain = root.path().join(format!("plain-message-{index}.db"));
        let conn = encrypted_sqlite::sqlite(&plain);
        conn.execute_batch(&format!(
            "CREATE TABLE Name2Id(user_name TEXT);
             CREATE TABLE [{table}](local_id INTEGER, local_type INTEGER, create_time INTEGER,
                real_sender_id INTEGER, message_content TEXT, WCDB_CT_message_content INTEGER);"
        ))
        .unwrap();
        conn.execute("INSERT INTO Name2Id(rowid,user_name) VALUES(1,?1)", [PEER])
            .unwrap();
        for (id, delta, kind) in rows {
            conn.execute(
                &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,1,?4,0)"),
                params![
                    id,
                    kind,
                    BASE + delta,
                    format!("synthetic shard {index}, message {id}")
                ],
            )
            .unwrap();
        }
        if index == 0 {
            if let Some((id, delta, kind, content)) = extra {
                conn.execute(
                    &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,1,?4,0)"),
                    params![id, kind, BASE + delta, content],
                )
                .unwrap();
            }
        }
        drop(conn);
        encrypted_sqlite::encrypt(&plain, &storage.join(source.replace('\\', "/")));
        keys.insert(source.to_owned(), "11".repeat(32));
    }
    let plain = root.path().join("plain-session.db");
    let conn = encrypted_sqlite::sqlite(&plain);
    conn.execute_batch("CREATE TABLE SessionTable(username TEXT,last_timestamp INTEGER)")
        .unwrap();
    conn.execute(
        "INSERT INTO SessionTable VALUES(?1,?2)",
        params![PEER, BASE + 500],
    )
    .unwrap();
    drop(conn);
    encrypted_sqlite::encrypt(&plain, &storage.join("session/session.db"));
    keys.insert("session/session.db".into(), "11".repeat(32));
    let db = DbCache::with_dirs(storage, cache.clone(), cache.join("_mtimes.json"), keys)
        .await
        .unwrap();
    let names = Names {
        map: HashMap::from([(PEER.into(), "Synthetic peer".into())]),
        md5_to_uname: HashMap::from([(
            format!("{:x}", md5::compute(PEER.as_bytes())),
            PEER.into(),
        )]),
        msg_db_keys: vec![SOURCE_1.into(), RAW_SOURCE_0.into()],
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    };
    Fixture {
        _root: root,
        db,
        names,
    }
}

fn assert_identities(response: &Value, expected: &[(i64, i64, &str)]) {
    let rows = response["messages"].as_array().expect("query messages");
    assert_eq!(response["count"].as_u64(), Some(expected.len() as u64));
    assert_eq!(rows.len(), expected.len());
    assert!(response["meta"]["shard_paths"].is_null());
    let mut actual = Vec::new();
    for row in rows {
        let source = row["source"].as_str().expect("logical source");
        assert!(
            matches!(source, SOURCE_0 | SOURCE_1),
            "unexpected source: {source}"
        );
        assert!(!Path::new(source).is_absolute());
        assert!(!source.contains('\\') && !source.contains(':') && !source.starts_with('/'));
        actual.push((
            row["local_id"].as_i64().expect("actual local_id"),
            row["timestamp"].as_i64().expect("actual timestamp"),
            source.to_owned(),
        ));
    }
    assert!(actual.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    // 不要求同秒消息的跨分片顺序；必须完整保留每个实际身份，包括重复 local_id。
    actual.sort();
    let mut expected: Vec<_> = expected
        .iter()
        .map(|(id, delta, source)| (*id, BASE + delta, (*source).to_owned()))
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn history_ordinary_preserves_source_and_duplicate_local_ids() {
    let f = fixture().await;
    let result = q_history(
        &f.db, &f.names, PEER, 10, 0, None, None, None, false, false, None, false,
    )
    .await
    .unwrap();
    assert_identities(
        &result,
        &[
            (17, 100, SOURCE_0),
            (7, 200, SOURCE_0),
            (7, 200, SOURCE_1),
            (29, 300, SOURCE_1),
            (91, 400, SOURCE_1),
            (44, 500, SOURCE_0),
        ],
    );
}

#[tokio::test]
async fn history_multitype_preserves_source_after_global_paging() {
    let f = fixture().await;
    let result = q_history(
        &f.db,
        &f.names,
        PEER,
        3,
        1,
        None,
        None,
        None,
        false,
        false,
        Some(&[1, 3]),
        false,
    )
    .await
    .unwrap();
    assert_identities(
        &result,
        &[(7, 200, SOURCE_0), (7, 200, SOURCE_1), (29, 300, SOURCE_1)],
    );
}

#[tokio::test]
async fn history_oldest_preserves_source_and_duplicate_local_ids() {
    let f = fixture().await;
    let result = q_history(
        &f.db, &f.names, PEER, 3, 0, None, None, None, false, false, None, true,
    )
    .await
    .unwrap();
    assert_identities(
        &result,
        &[(17, 100, SOURCE_0), (7, 200, SOURCE_0), (7, 200, SOURCE_1)],
    );
}

#[tokio::test]
async fn new_messages_preserves_source_across_incremental_pages() {
    let f = fixture().await;
    let first = q_new_messages(
        &f.db,
        &f.names,
        Some(HashMap::from([(PEER.into(), BASE + 100)])),
        2,
        false,
        false,
    )
    .await
    .unwrap();
    assert_identities(&first, &[(7, 200, SOURCE_0), (7, 200, SOURCE_1)]);
    assert_eq!(first["new_state"][PEER].as_i64(), Some(BASE + 200));
    let state = serde_json::from_value(first["new_state"].clone()).unwrap();
    let second = q_new_messages(&f.db, &f.names, Some(state), 10, false, false)
        .await
        .unwrap();
    assert_identities(
        &second,
        &[
            (29, 300, SOURCE_1),
            (91, 400, SOURCE_1),
            (44, 500, SOURCE_0),
        ],
    );
    assert_eq!(second["new_state"][PEER].as_i64(), Some(BASE + 500));
    let state = serde_json::from_value(second["new_state"].clone()).unwrap();
    let empty = q_new_messages(&f.db, &f.names, Some(state), 10, false, false)
        .await
        .unwrap();
    assert_identities(&empty, &[]);
    assert_eq!(empty["new_state"][PEER].as_i64(), Some(BASE + 500));
}

#[tokio::test]
async fn history_and_new_messages_preserve_rich_link_and_original_fields() {
    let xml = concat!(
        "<msg><appmsg><type>5</type><title>Synthetic article</title>",
        "<des>Fixture preview</des><sourcedisplayname>Fixture publisher</sourcedisplayname>",
        "<url>https://example.com/article</url></appmsg></msg>"
    );
    let f = fixture_with_extra(Some((101, 450, 49, xml))).await;
    let history = q_history(
        &f.db,
        &f.names,
        PEER,
        10,
        0,
        None,
        None,
        None,
        false,
        false,
        Some(&[49]),
        false,
    )
    .await
    .unwrap();
    assert_identities(&history, &[(101, 450, SOURCE_0)]);
    let incremental = q_new_messages(
        &f.db,
        &f.names,
        Some(HashMap::from([(PEER.into(), BASE + 400)])),
        10,
        false,
        false,
    )
    .await
    .unwrap();
    assert_identities(&incremental, &[(101, 450, SOURCE_0), (44, 500, SOURCE_0)]);
    for response in [&history, &incremental] {
        let row = response["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["local_id"].as_i64() == Some(101))
            .expect("actual rich link row");
        assert_eq!(row["source"], SOURCE_0);
        assert_eq!(row["local_id"], 101);
        assert_eq!(row["timestamp"], BASE + 450);
        assert_eq!(row["content"], "[链接] Synthetic article");
        assert_eq!(row["rich"]["type"], "link");
        assert_eq!(row["rich"]["title"], "Synthetic article");
        assert_eq!(row["rich"]["des"], "Fixture preview");
        assert_eq!(row["rich"]["source"], "Fixture publisher");
        assert_eq!(row["rich"]["url"], "https://example.com/article");
    }
}
