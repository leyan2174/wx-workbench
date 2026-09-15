use crate::adapters::wechat::media::voice_catalog as catalog;
use crate::business::voice::catalog as domain;

fn legacy_query(
    shards: &[catalog::MediaShard],
    query: &domain::Query,
) -> anyhow::Result<Vec<catalog::LegacyVoiceMessage>> {
    catalog::legacy_rows(&domain::list(&catalog::Catalog::new(shards), query)?)
}

use crate::{daemon::cache::DbCache, database_media as dm, mcp_voice as mv};
use rusqlite::{params, Connection};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const SILK: &[u8] = b"#!SILK_V3synthetic-not-decoded";

fn query() -> domain::Query {
    domain::Query {
        username: "alice".into(),
        limit: 100,
        offset: 0,
        since: None,
        until: None,
    }
}

fn media(root: &Path, name: &str, alice: i64, bob: i64) -> catalog::MediaShard {
    fs::create_dir_all(root.join("message")).unwrap();
    let path = root.join("message").join(name);
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE Name2Id(user_name TEXT COLLATE NOCASE);
        CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB);").unwrap();
    c.execute(
        "INSERT INTO Name2Id(rowid,user_name) VALUES(?1,'alice'),(?2,'bob')",
        params![alice, bob],
    )
    .unwrap();
    catalog::MediaShard {
        source: format!("message/{}", name.to_lowercase()),
        path,
    }
}

fn voice(
    shard: &catalog::MediaShard,
    owner: i64,
    id: i64,
    time: i64,
    server: i64,
    data: Option<&[u8]>,
) {
    Connection::open(&shard.path)
        .unwrap()
        .execute(
            "INSERT INTO VoiceInfo VALUES(?1,?2,?3,?4,?5)",
            params![owner, id, time, server, data],
        )
        .unwrap();
}

fn message(root: &Path) {
    let c = Connection::open(root.join("message/message_0.db")).unwrap();
    let table = format!("Msg_{:x}", md5::compute("alice"));
    c.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER);
        INSERT INTO [{table}] VALUES(7,34,100,900)")).unwrap();
}

fn resolve(root: &Path) -> Result<dm::DatabaseVoice, dm::DatabaseMediaError> {
    dm::resolve_voice(
        root,
        dm::MessageIdentity {
            username: "alice",
            source: "message/message_0.db",
            local_id: 7,
        },
    )
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(root.join("message"))
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            let bytes = fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn security_library_local_name_ids_and_binary_names_do_not_cross_contacts() {
    let d = tempfile::tempdir().unwrap();
    let a = media(d.path(), "media_0.db", 1, 9);
    let b = media(d.path(), "media_1.db", 9, 1);
    voice(&a, 1, 701, 100, 900, Some(SILK));
    voice(&a, 9, 801, 100, 900, Some(b"bob-a"));
    voice(&b, 9, 702, 200, 901, Some(SILK));
    voice(&b, 1, 802, 100, 900, Some(b"bob-b"));
    message(d.path());
    let before = snapshot(d.path());
    let rows = legacy_query(&[b.clone(), a], &query()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.local_id, r.chat_name_id))
            .collect::<Vec<_>>(),
        [(702, 9), (701, 1)]
    );
    assert_eq!(resolve(d.path()).unwrap().evidence.media_local_id, 701);
    let mut q = query();
    q.username = "ALICE".into();
    assert!(legacy_query(&[b], &q).unwrap().is_empty());
    assert_eq!(before, snapshot(d.path()));
}

#[test]
fn security_pagination_matches_independent_oracle_for_ties_ranges_and_permutations() {
    let d = tempfile::tempdir().unwrap();
    let mut shards = Vec::new();
    let mut oracle = Vec::new();
    for n in [2, 10, 0] {
        let s = media(d.path(), &format!("media_{n}.db"), n + 1, n + 100);
        for i in 0..9 {
            let time = (i % 4) * 100 - 100;
            let id = i % 3;
            voice(
                &s,
                n + 1,
                id,
                time,
                i,
                if i == 0 { None } else { Some(b"") },
            );
            oracle.push((time, s.source.clone(), id, i + 1));
        }
        shards.push(s);
    }
    oracle.sort_by_key(|(time, source, id, rowid)| {
        (
            std::cmp::Reverse(*time),
            source.clone(),
            std::cmp::Reverse(*id),
            std::cmp::Reverse(*rowid),
        )
    });
    let mut cases = 0;
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let selected: Vec<_> = order.map(|i| shards[i].clone()).into();
        for (since, until) in [
            (None, None),
            (Some(0), None),
            (None, Some(0)),
            (Some(0), Some(0)),
            (Some(-100), Some(100)),
        ] {
            for limit in [1, 2, 7, 40] {
                for offset in [0, 1, 6, 26, 27, 40] {
                    let q = domain::Query {
                        since,
                        until,
                        limit,
                        offset,
                        ..query()
                    };
                    let expected: Vec<_> = oracle
                        .iter()
                        .filter(|r| {
                            since.is_none_or(|t| r.0 >= t) && until.is_none_or(|t| r.0 <= t)
                        })
                        .skip(offset)
                        .take(limit)
                        .cloned()
                        .collect();
                    let actual: Vec<_> = legacy_query(&selected, &q)
                        .unwrap()
                        .into_iter()
                        .map(|r| (r.create_time, r.source, r.local_id, r.media_rowid))
                        .collect();
                    assert_eq!(actual, expected);
                    cases += 1;
                }
            }
        }
    }
    println!("ORACLE: {cases} pagination/range/permutation cases passed");
}

#[test]
fn security_null_length_empty_blob_and_inclusive_zero_time_are_distinct() {
    let d = tempfile::tempdir().unwrap();
    let s = media(d.path(), "media_0.db", 1, 2);
    voice(&s, 1, 1, -1, 1, Some(b"x"));
    voice(&s, 1, 2, 0, 2, None);
    voice(&s, 1, 3, 0, 3, Some(b""));
    voice(&s, 1, 4, 1, 4, Some(b"\0\xff\0"));
    let q = domain::Query {
        since: Some(0),
        until: Some(0),
        ..query()
    };
    let rows = legacy_query(&[s.clone()], &q).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.local_id, r.voice_data_bytes))
            .collect::<Vec<_>>(),
        [(3, Some(0)), (2, None)]
    );
    assert_eq!(
        legacy_query(&[s], &query()).unwrap()[0].voice_data_bytes,
        Some(3)
    );
}

#[test]
fn security_old_listing_schema_without_server_id_remains_listable_only() {
    let d = tempfile::tempdir().unwrap();
    let s = media(d.path(), "media_0.db", 1, 2);
    voice(&s, 1, 700, 100, 900, Some(SILK));
    Connection::open(&s.path)
        .unwrap()
        .execute_batch("ALTER TABLE VoiceInfo DROP COLUMN svr_id")
        .unwrap();
    message(d.path());
    assert_eq!(legacy_query(&[s], &query()).unwrap()[0].local_id, 700);
    assert_eq!(
        resolve(d.path()).unwrap_err().kind,
        dm::ErrorKind::UnsupportedSchema
    );
}

#[tokio::test]
async fn security_adapter_rejects_missing_unknown_undecrypted_and_corrupt_shards() {
    let d = tempfile::tempdir().unwrap();
    let a = media(d.path(), "media_0.db", 1, 2);
    voice(&a, 1, 7, 100, 900, Some(SILK));
    let keys = vec![a.source.clone(), "message/media_1.db".into()];
    let mut db = DbCache {
        root: d.path().into(),
        keys: keys.clone(),
        paths: HashMap::from([(a.source.clone(), a.path.clone())]),
    };
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
    let b = media(d.path(), "media_1.db", 2, 1);
    db.keys = vec![a.source.clone()];
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
    db.keys = keys;
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
    db.paths.insert(b.source, b.path.clone());
    fs::write(&b.path, b"synthetic-corrupt").unwrap();
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
    message(d.path());
    assert!(resolve(d.path()).is_err());
}

#[test]
fn security_shadowed_name_rowid_is_rejected_before_cross_contact_attribution() {
    let d = tempfile::tempdir().unwrap();
    let s = media(d.path(), "media_0.db", 1, 2);
    voice(&s, 1, 701, 100, 900, Some(SILK));
    voice(&s, 2, 802, 100, 900, Some(b"bob-private"));
    let c = Connection::open(&s.path).unwrap();
    c.execute_batch(
        "ALTER TABLE Name2Id ADD COLUMN rowid INTEGER;
        UPDATE Name2Id SET rowid=CASE user_name WHEN 'alice' THEN 2 ELSE 1 END;",
    )
    .unwrap();
    let actual_owner: String = c
        .query_row("SELECT user_name FROM Name2Id WHERE _rowid_=2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(actual_owner, "bob");
    drop(c);
    message(d.path());
    let before = snapshot(d.path());
    // 威胁模型：普通 rowid 列伪装内部行号，企图把 Bob 的记录归给 Alice。
    let error = legacy_query(&[s], &query()).unwrap_err();
    assert!(format!("{error:#}").contains("unsupported rowid schema"));
    assert_eq!(
        resolve(d.path()).unwrap_err().kind,
        dm::ErrorKind::UnsupportedSchema
    );
    assert_eq!(before, snapshot(d.path()));
    println!("REGRESSION F1: shadowed Name2Id rowid is rejected by both MCP and ASR");
}

#[test]
fn security_uppercase_media_shard_cannot_hide_asr_ambiguity_on_windows() {
    let d = tempfile::tempdir().unwrap();
    let a = media(d.path(), "media_0.db", 1, 2);
    let b = media(d.path(), "MEDIA_1.DB", 9, 1);
    voice(&a, 1, 700, 100, 900, Some(SILK));
    voice(&b, 9, 701, 100, 900, Some(SILK));
    message(d.path());
    assert_eq!(legacy_query(&[a, b.clone()], &query()).unwrap().len(), 2);
    // 威胁模型：仅改变重复媒体片文件名大小写，不能绕过全片唯一性验证。
    let before = snapshot(d.path());
    assert_eq!(
        resolve(d.path()).unwrap_err().kind,
        dm::ErrorKind::AmbiguousMedia
    );
    assert_eq!(before, snapshot(d.path()));
    fs::rename(&b.path, d.path().join("message/media_2.db")).unwrap();
    assert_eq!(
        resolve(d.path()).unwrap_err().kind,
        dm::ErrorKind::AmbiguousMedia
    );
    println!("REGRESSION F2: uppercase and lowercase duplicate shards both produce AmbiguousMedia");
}

#[tokio::test]
async fn security_documented_incomplete_offline_inventory_cannot_prove_uniqueness() {
    let d = tempfile::tempdir().unwrap();
    let a = media(d.path(), "media_0.db", 1, 2);
    let b = media(d.path(), "media_1.db", 9, 1);
    voice(&a, 1, 700, 100, 900, Some(SILK));
    voice(&b, 9, 701, 100, 900, Some(SILK));
    message(d.path());
    assert_eq!(
        resolve(d.path()).unwrap_err().kind,
        dm::ErrorKind::AmbiguousMedia
    );
    assert_eq!(legacy_query(&[a.clone()], &query()).unwrap().len(), 1);
    fs::remove_file(&b.path).unwrap();
    assert!(resolve(d.path()).is_ok());
    let db = DbCache {
        root: d.path().into(),
        keys: vec![a.source.clone(), b.source],
        paths: HashMap::from([(a.source, a.path)]),
    };
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
    println!("BOUNDARY: offline caller must supply complete snapshot; account inventory adapter detects absent known shard");
}

#[tokio::test]
async fn security_adapter_preserves_raw_keys_and_rejects_canonical_duplicates() {
    let d = tempfile::tempdir().unwrap();
    let s = media(d.path(), "media_0.db", 1, 2);
    voice(&s, 1, 7, 100, 900, Some(SILK));
    let raw = "MESSAGE\\MEDIA_0.DB".to_owned();
    let mut db = DbCache {
        root: d.path().into(),
        keys: vec![raw.clone()],
        paths: HashMap::from([(raw, s.path)]),
    };
    let rows = mv::q_voice_messages(&db, &query()).await.unwrap();
    assert_eq!(
        catalog::legacy_rows(&rows).unwrap()[0].source,
        "message/media_0.db"
    );
    db.keys.push("message/media_0.db".into());
    assert!(mv::q_voice_messages(&db, &query()).await.is_err());
}
