use super::*;

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
            let error = query_voice_shards(std::slice::from_ref(&media), &query()).unwrap_err();
            assert!(
                format!("{error:#}").contains("shadows SQLite rowid"),
                "{table}/{alias}: {error:#}"
            );
            let mut absent = query();
            absent.username = "absent".into();
            assert!(query_voice_shards(std::slice::from_ref(&media), &absent).is_err());
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
    let error = query_voice_shards(&[media], &query()).unwrap_err();
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
                format!("{:#}", query_voice_shards(&[media], &query()).unwrap_err())
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
    let result = query_voice_shards(&[media], &query()).unwrap();
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
    let error = query_voice_shards(std::slice::from_ref(&a), &query()).unwrap_err();
    assert!(error.to_string().contains("message/media_0.db"));
    conn.execute("DROP TABLE Name2Id", []).unwrap();
    assert!(query_voice_shards(std::slice::from_ref(&a), &query())
        .unwrap_err()
        .to_string()
        .contains("message/media_0.db"));
    let b = shard(dir.path(), 1, &[]);
    let alias = MediaShard {
        source: "message/media_2.db".into(),
        path: b.path.clone(),
    };
    assert!(query_voice_shards(&[b, alias], &query()).is_err());
}

#[test]
fn absent_username_is_empty_not_another_chat_and_empty_blob_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let a = shard(dir.path(), 0, &[(1, 100, Some(b""))]);
    let mut q = query();
    let result = query_voice_shards(std::slice::from_ref(&a), &q).unwrap();
    assert_eq!(result[0].voice_data_bytes, Some(0));
    q.username = "Alice".into();
    assert!(query_voice_shards(std::slice::from_ref(&a), &q)
        .unwrap()
        .is_empty());
    q.username = "missing".into();
    assert!(query_voice_shards(&[a], &q).unwrap().is_empty());
}
fn query() -> VoiceQuery {
    VoiceQuery {
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
    let result = query_voice_shards(&[b.clone(), a.clone()], &q).unwrap();
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
        query_voice_shards(&[a, b], &q)
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
    assert!(query_voice_shards(&[a.clone(), bad.clone()], &query()).is_err());
    assert!(!bad.path.exists());
    std::fs::write(&bad.path, b"encrypted synthetic not SQLite").unwrap();
    assert!(query_voice_shards(&[a.clone(), bad], &query()).is_err());
    Connection::open(&a.path)
        .unwrap()
        .execute("INSERT INTO Name2Id VALUES ('alice')", [])
        .unwrap();
    assert!(query_voice_shards(&[a], &query()).is_err());
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
    assert!(query_voice_shards(&[a], &q).is_err());
}
