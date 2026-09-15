use super::*;
use crate::business::voice::catalog::{resolve_exact_chat, Query};
use std::collections::HashMap;

// Exercise the real typed source, then explicitly inspect the legacy wire view.
fn legacy_query(shards: &[MediaShard], query: &Query) -> Result<Vec<LegacyVoiceMessage>> {
    legacy_rows(&domain::list(&Catalog::new(shards), query)?)
}

#[test]
fn typed_previews_and_legacy_wire_golden_preserve_duplicate_ids() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[(7, 0, None), (7, 0, Some(b""))]);
    let mut b = shard(dir.path(), 1, &[(7, 0, Some(b"abc"))]);
    b.source = "MESSAGE\\MEDIA_1.DB".into();
    let shards = [b, a];
    let q = Query {
        since: Some(0),
        until: Some(0),
        ..query()
    };
    let page = domain::list(&Catalog::new(&shards), &q).unwrap();
    assert_eq!(page.entries.len(), 3);
    assert_eq!(
        page.entries.iter().map(|v| v.byte_len).collect::<Vec<_>>(),
        vec![Some(0), None, Some(3)]
    );
    for a in 0..3 {
        for b in a + 1..3 {
            assert_ne!(page.entries[a].source, page.entries[b].source);
        }
    }
    assert!(!format!("{page:?}").contains("media_"));
    let wire = serde_json::to_value(legacy_rows(&page).unwrap()).unwrap();
    let golden = serde_json::json!([
        {"username":"alice","source":"message/media_0.db","chat_name_id":1,
         "media_rowid":2,"local_id":7,"create_time":0,"voice_data_bytes":0},
        {"username":"alice","source":"message/media_0.db","chat_name_id":1,
         "media_rowid":1,"local_id":7,"create_time":0,"voice_data_bytes":null},
        {"username":"alice","source":"message/media_1.db","chat_name_id":1,
         "media_rowid":1,"local_id":7,"create_time":0,"voice_data_bytes":3}
    ]);
    assert_eq!(wire, golden);
    let paged = domain::list(
        &Catalog::new(&shards),
        &Query {
            limit: 1,
            offset: 1,
            ..q
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(legacy_rows(&paged).unwrap()).unwrap(),
        serde_json::json!([golden[1].clone()])
    );
}

#[test]
fn legacy_projection_rejects_foreign_or_changed_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let shards = [shard(dir.path(), 0, &[(7, 0, None)])];
    let page = domain::list(&Catalog::new(&shards), &query()).unwrap();
    for defect in 0..4 {
        let mut changed = page.clone();
        match defect {
            0 => changed.entries[0].source = SourceRef::new(()),
            1 => changed.entries[0].username = "bob".into(),
            2 => changed.entries[0].timestamp += 1,
            _ => changed.entries[0].byte_len = Some(0),
        }
        assert!(legacy_rows(&changed).is_err());
    }
}

#[test]
fn sqlite_pagination_range_is_adapter_owned_and_checked_before_io() {
    let q = Query {
        limit: i64::MAX as usize,
        offset: 1,
        ..query()
    };
    assert!(q.candidate_limit().is_ok());
    let error = domain::list(&Catalog::new(&[]), &q).unwrap_err();
    assert_eq!(error.to_string(), "pagination exceeds SQLite integer range");
}

#[test]
fn shadowed_rowid_aliases_are_rejected_before_attribution() {
    for table in ["Name2Id", "VoiceInfo"] {
        for alias in ["rowid", "_rowid_", "oid", "RoWiD", "_ROWID_", "OID"] {
            let dir = tempfile::tempdir().unwrap();
            let media = shard(dir.path(), 0, &[(701, 100, Some(b"alice"))]);
            let conn = Connection::open(&media.path).unwrap();
            // 同复现：将 Alice 的用户列冒充为 Bob 的内部行号 2。
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN \"{alias}\" INTEGER; UPDATE {table} SET \"{alias}\"=2;")).unwrap();
            drop(conn);
            let before = std::fs::read(&media.path).unwrap();
            let error = legacy_query(std::slice::from_ref(&media), &query()).unwrap_err();
            assert!(
                format!("{error:#}").contains("shadows SQLite rowid"),
                "{table}/{alias}: {error:#}"
            );
            let mut absent = query();
            absent.username = "absent".into();
            assert!(legacy_query(std::slice::from_ref(&media), &absent).is_err());
            assert_eq!(std::fs::read(media.path).unwrap(), before);
        }
    }
}

#[test]
fn generated_rowid_column_is_also_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let media = shard(dir.path(), 0, &[]);
    Connection::open(&media.path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE Name2Id ADD COLUMN rowid INTEGER GENERATED ALWAYS AS (2) VIRTUAL;",
        )
        .unwrap();
    let error = legacy_query(&[media], &query()).unwrap_err();
    assert!(format!("{error:#}").contains("shadows SQLite rowid"));
}

#[test]
fn views_and_without_rowid_tables_are_not_identity_evidence() {
    for table in ["Name2Id", "VoiceInfo"] {
        for variant in ["view", "without"] {
            let dir = tempfile::tempdir().unwrap();
            let media = shard(dir.path(), 0, &[]);
            let conn = Connection::open(&media.path).unwrap();
            let ddl = if variant == "view" {
                format!("ALTER TABLE {table} RENAME TO original_table; CREATE VIEW {table} AS SELECT rowid,* FROM original_table;")
            } else if table == "Name2Id" {
                "DROP TABLE Name2Id; CREATE TABLE Name2Id(user_name TEXT PRIMARY KEY) WITHOUT ROWID;".into()
            } else {
                "DROP TABLE VoiceInfo; CREATE TABLE VoiceInfo(local_id INTEGER PRIMARY KEY,chat_name_id INTEGER,create_time INTEGER,voice_data BLOB) WITHOUT ROWID;".into()
            };
            conn.execute_batch(&ddl).unwrap();
            drop(conn);
            assert!(
                format!("{:#}", legacy_query(&[media], &query()).unwrap_err())
                    .contains("ordinary rowid table")
            );
        }
    }
}

#[test]
fn ordinary_tables_and_nonreserved_integer_primary_keys_stay_supported() {
    let dir = tempfile::tempdir().unwrap();
    let media = shard(dir.path(), 0, &[]);
    let conn = Connection::open(&media.path).unwrap();
    conn.execute_batch("DROP TABLE Name2Id; DROP TABLE VoiceInfo;
        CREATE TABLE Name2Id(id INTEGER PRIMARY KEY,user_name TEXT,extra TEXT);
        INSERT INTO Name2Id VALUES(10,'alice','keep'),(20,'bob','keep');
        CREATE TABLE VoiceInfo(id INTEGER PRIMARY KEY,chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,voice_data BLOB);
        INSERT INTO VoiceInfo VALUES(101,10,701,100,zeroblob(5)),(102,20,802,200,zeroblob(9));").unwrap();
    drop(conn);
    let result = legacy_query(&[media], &query()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        (
            result[0].chat_name_id,
            result[0].media_rowid,
            result[0].local_id
        ),
        (10, 101, 701)
    );
}

#[test]
fn media_key_classification_is_strict_and_canonical() {
    for raw in ["message/media_0.db", "message\\MEDIA_12.DB"] {
        assert!(source_key(raw).is_ok());
    }
    for raw in [
        "message/message_0.db",
        "message/media_cache.db",
        "message/media_.db",
        "media_0.db",
        "message/media_0.db-wal",
        "message/media_0.db-shm",
        "contact/media_0.db",
        "message/media_1/../0.db",
        "message/media_１.db",
    ] {
        assert!(source_key(raw).is_err(), "{raw}");
    }
}

#[test]
fn missing_name_table_bad_blob_and_aliases_fail_with_source() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[(1, 100, Some(b"a"))]);
    let conn = Connection::open(&a.path).unwrap();
    conn.execute(
        "UPDATE VoiceInfo SET voice_data='not a blob' WHERE chat_name_id=1",
        [],
    )
    .unwrap();
    let error = legacy_query(std::slice::from_ref(&a), &query()).unwrap_err();
    assert!(error.to_string().contains("message/media_0.db"));
    conn.execute("DROP TABLE Name2Id", []).unwrap();
    assert!(legacy_query(std::slice::from_ref(&a), &query())
        .unwrap_err()
        .to_string()
        .contains("message/media_0.db"));
    let b = shard(dir.path(), 1, &[]);
    let alias = MediaShard {
        source: "message/media_2.db".into(),
        path: b.path.clone(),
    };
    assert!(legacy_query(&[b, alias], &query()).is_err());
}

#[test]
fn absent_username_is_empty_not_another_chat_and_empty_blob_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[(1, 100, Some(b""))]);
    let mut q = query();
    let result = legacy_query(std::slice::from_ref(&a), &q).unwrap();
    assert_eq!(result[0].voice_data_bytes, Some(0));
    q.username = "Alice".into();
    assert!(legacy_query(std::slice::from_ref(&a), &q)
        .unwrap()
        .is_empty());
    q.username = "missing".into();
    assert!(legacy_query(&[a], &q).unwrap().is_empty());
}
fn query() -> Query {
    Query {
        username: "alice".into(),
        limit: 20,
        offset: 0,
        since: None,
        until: None,
    }
}
fn shard(root: &Path, n: usize, values: &[(i64, i64, Option<&[u8]>)]) -> MediaShard {
    let path = root.join(format!("media_{n}.db"));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('alice'),('bob');
        CREATE TABLE VoiceInfo(chat_name_id INTEGER, local_id INTEGER, create_time INTEGER, voice_data BLOB);").unwrap();
    for (id, time, data) in values {
        conn.execute(
            "INSERT INTO VoiceInfo VALUES(1,?1,?2,?3)",
            rusqlite::params![id, time, data],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO VoiceInfo VALUES(2,999,9999,zeroblob(500))", [])
        .unwrap();
    MediaShard {
        source: format!("message/media_{n}.db"),
        path,
    }
}
#[test]
fn global_desc_pagination_and_exact_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(
        dir.path(),
        0,
        &[(1, 100, Some(b"abc")), (2, 300, Some(b""))],
    );
    let b = shard(dir.path(), 1, &[(1, 200, None), (2, 300, Some(b"abcdef"))]);
    let before = std::fs::read(&a.path).unwrap();
    let mut q = query();
    q.limit = 2;
    q.offset = 1;
    let result = legacy_query(&[b.clone(), a.clone()], &q).unwrap();
    assert_eq!(
        (
            result[0].create_time,
            result[0].source.as_str(),
            result[0].voice_data_bytes
        ),
        (300, "message/media_1.db", Some(6))
    );
    assert_eq!(
        (result[1].create_time, result[1].voice_data_bytes),
        (200, None)
    );
    assert_eq!(std::fs::read(&a.path).unwrap(), before);
    q.since = Some(100);
    q.until = Some(200);
    q.offset = 0;
    assert_eq!(
        legacy_query(&[a, b], &q)
            .unwrap()
            .iter()
            .map(|r| r.create_time)
            .collect::<Vec<_>>(),
        [200, 100]
    );
}
#[test]
fn missing_corrupt_or_ambiguous_shards_fail_whole_query() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[(1, 100, Some(b"a"))]);
    let bad = MediaShard {
        source: "message/media_1.db".into(),
        path: dir.path().join("bad.db"),
    };
    assert!(legacy_query(&[a.clone(), bad.clone()], &query()).is_err());
    assert!(!bad.path.exists());
    std::fs::write(&bad.path, b"encrypted synthetic not SQLite").unwrap();
    assert!(legacy_query(&[a.clone(), bad], &query()).is_err());
    Connection::open(&a.path)
        .unwrap()
        .execute("INSERT INTO Name2Id VALUES ('alice')", [])
        .unwrap();
    assert!(legacy_query(&[a], &query()).is_err());
}
#[test]
fn exact_chat_and_invalid_pagination() {
    let names = HashMap::from([
        ("alice".into(), "同名".into()),
        ("bob".into(), "同名".into()),
    ]);
    assert_eq!(resolve_exact_chat("alice", &names).unwrap(), "alice");
    assert!(resolve_exact_chat("同名", &names).is_err());
    assert!(resolve_exact_chat("ali", &names).is_err());
    let mut q = query();
    q.limit = 0;
    assert!(q.candidate_limit().is_err());
    q.limit = 20;
    q.offset = usize::MAX;
    assert!(q.candidate_limit().is_err());
    q.offset = 0;
    q.since = Some(2);
    q.until = Some(1);
    assert!(q.candidate_limit().is_err());
}
#[test]
fn schema_failure_is_not_hidden_by_missing_contact() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[]);
    Connection::open(&a.path)
        .unwrap()
        .execute("DROP TABLE VoiceInfo", [])
        .unwrap();
    let mut q = query();
    q.username = "absent".into();
    assert!(legacy_query(&[a], &q).is_err());
}
