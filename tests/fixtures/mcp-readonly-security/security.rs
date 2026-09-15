use mcp_readonly_security_harness::business::contacts as contact_domain;
use mcp_readonly_security_harness::{contacts, refer, DbCache, Names};
use rusqlite::{params, types::Value as SqlValue, Connection};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const SECRET: &str = "SYNTHETIC_SECRET_CDN_AES_DO_NOT_ECHO";

fn reply() -> String {
    format!("<msg><appmsg><title>safe reply</title><type>57</type><refermsg><type>3</type><fromusr>alice</fromusr><displayname>Alice</displayname><content>&lt;img aeskey='{SECRET}' cdnurl='{SECRET}'/&gt;</content></refermsg></appmsg></msg>")
}

fn table() -> String {
    format!("Msg_{:x}", md5::compute("alice"))
}

fn create_message(path: &Path) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(&format!("CREATE TABLE [{}](local_id INTEGER,local_type,create_time,WCDB_CT_message_content,message_content)", table())).unwrap();
}

fn insert_message(path: &Path, id: i64, time: i64, kind: i64, compression: i64, body: SqlValue) {
    Connection::open(path)
        .unwrap()
        .execute(
            &format!("INSERT INTO [{}] VALUES(?1,?2,?3,?4,?5)", table()),
            params![id, kind, time, compression, body],
        )
        .unwrap();
}

struct Messages {
    _temp: tempfile::TempDir,
    db: DbCache,
    names: Names,
    paths: Vec<PathBuf>,
}

impl Messages {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("wxid_self_abcd/db_storage");
        fs::create_dir_all(root.join("message")).unwrap();
        let mut db = DbCache::new(root);
        let mut names = Names {
            map: HashMap::from([
                ("alice".into(), "Alice".into()),
                ("bob".into(), "Bob".into()),
            ]),
            ..Names::default()
        };
        let mut paths = Vec::new();
        for n in 0..count {
            let key = format!("message/message_{n}.db");
            let path = db.root.join(&key);
            create_message(&path);
            db.paths.insert(key.clone(), path.clone());
            names.msg_db_keys.push(key);
            paths.push(path);
        }
        Self {
            _temp: temp,
            db,
            names,
            paths,
        }
    }

    async fn query(&self, chat: &str, id: i64, time: i64) -> anyhow::Result<Value> {
        refer::q_decode_refer(&self.db, &self.names, chat, id, time).await
    }

    fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(self.db.root.join("message"))
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
}

#[tokio::test]
async fn refer_contact_ambiguity_is_rejected_after_exact_directory_check() {
    let mut f = Messages::new(1);
    f.names.map.insert("alice".into(), "Shared North".into());
    f.names.map.insert("bob".into(), "Shared South".into());
    for query in ["Shared", "shared"] {
        let value = f.query(query, 7, 0).await.unwrap();
        assert_eq!(value["exit_code"], 2);
        assert!(value.get("refer").is_none());
    }
    f.names.map.insert("bob".into(), "SHARED NORTH".into());
    assert_eq!(f.query("Shared North", 7, 0).await.unwrap()["exit_code"], 2);
    // Exact session/table evidence must win over an ambiguous display name.
    // Directory reads are permitted here, but no message is returned for ambiguity.
    assert!(!f.db.requests.lock().unwrap().is_empty());
    assert!(f.db.scan_count() > 0);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    assert_eq!(f.query("alice", 7, 0).await.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn refer_localid_uniqueness_spans_shards_before_type_filtering() {
    let f = Messages::new(2);
    insert_message(&f.paths[0], 7, 0, 49, 0, SqlValue::Text(reply()));
    insert_message(
        &f.paths[1],
        7,
        100,
        1,
        0,
        SqlValue::Text("not reply".into()),
    );
    let before = f.snapshot();
    let ambiguous = f.query("alice", 7, 0).await.unwrap();
    assert_eq!(ambiguous["exit_code"], 2);
    assert!(ambiguous.get("refer").is_none());
    assert_eq!(f.query("alice", 7, 100).await.unwrap()["exit_code"], 1);
    insert_message(&f.paths[1], 8, 200, 49, 0, SqlValue::Text(reply()));
    insert_message(&f.paths[0], 8, 200, 49, 0, SqlValue::Text(reply()));
    assert_eq!(f.query("alice", 8, 200).await.unwrap()["exit_code"], 2);
    assert!(!before.is_empty());
    let before = f.snapshot();
    assert_eq!(f.query("alice", 8, 0).await.unwrap()["exit_code"], 2);
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn refer_known_missing_corrupt_and_new_lowercase_shards_never_succeed_partially() {
    for mode in ["missing", "corrupt", "new", "schema"] {
        let f = Messages::new(2);
        insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
        match mode {
            "missing" => fs::remove_file(&f.paths[1]).unwrap(),
            "corrupt" => fs::write(&f.paths[1], b"synthetic corrupt sqlite").unwrap(),
            "schema" => Connection::open(&f.paths[1])
                .unwrap()
                .execute_batch(&format!(
                    "ALTER TABLE [{}] DROP COLUMN message_content",
                    table()
                ))
                .unwrap(),
            _ => create_message(&f.db.root.join("message/message_9.db")),
        }
        let before = f.snapshot();
        assert!(f.query("alice", 7, 0).await.is_err(), "{mode}");
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn refer_new_shard_after_first_inventory_scan_is_rejected() {
    let f = Messages::new(1);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    let staged = f._temp.path().join("staged.db");
    create_message(&staged);
    insert_message(&staged, 7, 100, 49, 0, SqlValue::Text(reply()));
    *f.db.publish_on_get.lock().unwrap() =
        Some((1, staged, f.db.root.join("message/message_1.db")));
    let err = f.query("alice", 7, 0).await.unwrap_err();
    assert!(format!("{err:#}").contains("unknown"));
    assert_eq!(f.db.scan_count(), 2);
}

#[tokio::test]
async fn refer_compressed_xml_and_secret_errors_are_bounded_and_redacted() {
    let f = Messages::new(1);
    let bodies = [
        (
            SqlValue::Blob(zstd::encode_all(reply().as_bytes(), 1).unwrap()),
            4,
            true,
        ),
        (SqlValue::Text(reply()), 4, true),
        (
            SqlValue::Blob(format!("{SECRET} invalid zstd").into_bytes()),
            4,
            false,
        ),
        (
            SqlValue::Text(format!(
                "<!DOCTYPE msg [<!ENTITY x '{SECRET}'>]><msg>&x;</msg>"
            )),
            0,
            false,
        ),
        (SqlValue::Text(format!("<msg><appmsg>{SECRET}")), 0, false),
        (
            SqlValue::Blob(zstd::encode_all("x".repeat(131_073).as_bytes(), 1).unwrap()),
            4,
            false,
        ),
        (
            SqlValue::Text(reply().replace("safe reply", &"x".repeat(20_001))),
            0,
            false,
        ),
    ];
    for (i, (body, compression, valid)) in bodies.into_iter().enumerate() {
        insert_message(&f.paths[0], i as i64, 100, 49, compression, body);
        let before = f.snapshot();
        let value = f.query("alice", i as i64, 100).await.unwrap();
        assert_eq!(value["exit_code"], if valid { 0 } else { 1 });
        assert!(!value.to_string().contains(SECRET));
        assert_eq!(value.get("refer").is_some(), valid);
        assert_eq!(before, f.snapshot());
    }
    insert_message(
        &f.paths[0],
        100,
        100,
        49,
        0,
        SqlValue::Text(format!("{SECRET}{}", "x".repeat(1_048_577))),
    );
    let err = f.query("alice", 100, 100).await.unwrap_err();
    assert!(format!("{err:#}").contains("stored byte limit"));
    assert!(!format!("{err:#}").contains(SECRET));
}

#[tokio::test]
async fn uppercase_unknown_shard_rejects_duplicate_identity_without_mutation() {
    let f = Messages::new(1);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    let unknown = f.db.root.join("message/MESSAGE_1.DB");
    create_message(&unknown);
    insert_message(&unknown, 7, 100, 49, 0, SqlValue::Text(reply()));
    let before = f.snapshot();
    let error = f.query("alice", 7, 100).await.unwrap_err();
    assert!(format!("{error:#}").contains("unknown message shards"));
    assert_eq!(before, f.snapshot());
    fs::rename(&unknown, f.db.root.join("message/message_2.db")).unwrap();
    let before = f.snapshot();
    assert!(f.query("alice", 7, 100).await.is_err());
    assert_eq!(before, f.snapshot());
    println!("REGRESSION R1: real checked inventory rejects uppercase and lowercase unknown shards without source mutation");
}

#[tokio::test]
async fn target_named_view_is_rejected_without_mutation() {
    let f = Messages::new(2);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    insert_message(&f.paths[1], 7, 100, 49, 0, SqlValue::Text(reply()));
    assert_eq!(f.query("alice", 7, 100).await.unwrap()["exit_code"], 2);
    let c = Connection::open(&f.paths[1]).unwrap();
    c.execute_batch(&format!(
        "ALTER TABLE [{}] RENAME TO PreservedRows; CREATE VIEW [{}] AS SELECT * FROM PreservedRows",
        table(),
        table()
    ))
    .unwrap();
    drop(c);
    let before = f.snapshot();
    let error = f.query("alice", 7, 100).await.unwrap_err();
    assert!(format!("{error:#}").contains("unsupported message table schema"));
    assert_eq!(before, f.snapshot());
    println!("REGRESSION R2: unsupported exact target view is rejected without source mutation");
}

#[tokio::test]
async fn checked_inventory_accepts_raw_known_case_and_propagates_missing_directory() {
    let mut f = Messages::new(1);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    let raw = "MESSAGE\\MESSAGE_0.DB".to_owned();
    f.db.paths = HashMap::from([(raw.clone(), f.paths[0].clone())]);
    f.names.msg_db_keys = vec![raw.clone()];
    let before = f.snapshot();
    assert_eq!(f.query("alice", 7, 100).await.unwrap()["exit_code"], 0);
    assert_eq!(*f.db.requests.lock().unwrap(), [raw]);
    assert_eq!(before, f.snapshot());
    let empty = Messages::new(0);
    fs::remove_dir(empty.db.root.join("message")).unwrap();
    assert!(empty.query("alice", 7, 100).await.is_err());
    assert!(empty.db.requests.lock().unwrap().is_empty());
    assert!(!empty.db.root.join("message").exists());
}

#[tokio::test]
async fn checked_inventory_rejects_directory_disguised_as_message_shard() {
    let f = Messages::new(1);
    insert_message(&f.paths[0], 7, 100, 49, 0, SqlValue::Text(reply()));
    let before = fs::read(&f.paths[0]).unwrap();
    fs::create_dir(f.db.root.join("message/MESSAGE_9.DB")).unwrap();
    let error = f.query("alice", 7, 100).await.unwrap_err();
    assert!(format!("{error:#}").contains("not a regular file"));
    assert_eq!(before, fs::read(&f.paths[0]).unwrap());
    assert!(f.db.requests.lock().unwrap().is_empty());
}

fn contact_db() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("contact.db");
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE contact_label(label_id_,label_name_,sort_order_); CREATE TABLE contact(username TEXT,extra_buffer)").unwrap();
    (temp, path)
}

fn varint(mut n: usize, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

fn labels(ids: &str) -> Vec<u8> {
    let mut buffer = vec![242, 1];
    varint(ids.len(), &mut buffer);
    buffer.extend_from_slice(ids.as_bytes());
    buffer
}

fn load_tags(
    path: &Path,
    names: &HashMap<String, String>,
) -> contact_domain::Result<Vec<contact_domain::Tag>> {
    use contact_domain::ContactSource;
    let mut source = mcp_readonly_security_harness::adapters::wechat::contacts::SqliteContacts::new(
        path.to_owned(),
    );
    source.display_names = names.clone();
    source.tags()
}

fn select_tag<'a>(
    tags: &'a [contact_domain::Tag],
    query: &str,
) -> contact_domain::Result<&'a contact_domain::Tag> {
    let index = contact_domain::select_name(tags.iter().map(|tag| tag.name.as_str()), query)?;
    Ok(&tags[index])
}

#[test]
fn tags_numeric_ids_and_duplicate_associations_preserve_declared_contract() {
    let (_temp, path) = contact_db();
    let c = Connection::open(&path).unwrap();
    for (id, name, order) in [
        (SqlValue::Integer(1), "old", 1),
        (SqlValue::Real(1.0), "numeric", 2),
        (SqlValue::Text("1".into()), "text", 3),
        (SqlValue::Integer(9_007_199_254_740_993), "large-int", 4),
        (SqlValue::Real(9_007_199_254_740_992.0), "large-real", 5),
    ] {
        c.execute(
            "INSERT INTO contact_label VALUES(?1,?2,?3)",
            params![id, name, order],
        )
        .unwrap();
    }
    for user in ["alice", "alice", "bob"] {
        c.execute(
            "INSERT INTO contact VALUES(?1,?2)",
            params![user, labels("1,1,9007199254740993,9007199254740992")],
        )
        .unwrap();
    }
    drop(c);
    let before = fs::read(&path).unwrap();
    let tags = load_tags(
        &path,
        &HashMap::from([
            ("alice".into(), "Same".into()),
            ("bob".into(), "Same".into()),
        ]),
    )
    .unwrap();
    assert_eq!(tags.len(), 4);
    assert_eq!(tags.iter().map(|tag| tag.members.len()).sum::<usize>(), 12);
    assert_eq!(select_tag(&tags, "numeric").unwrap().members.len(), 6);
    assert_eq!(select_tag(&tags, "text").unwrap().members.len(), 0);
    assert_eq!(select_tag(&tags, "large-int").unwrap().members.len(), 3);
    assert_eq!(select_tag(&tags, "large-real").unwrap().members.len(), 3);
    let members = &select_tag(&tags, "numeric").unwrap().members;
    assert_eq!(members.iter().filter(|m| m.id.0 == "bob").count(), 2);
    assert_eq!(before, fs::read(&path).unwrap());
}

#[test]
fn tags_exact_and_fuzzy_ambiguity_reject_without_selecting_a_member() {
    let (_temp, path) = contact_db();
    Connection::open(&path).unwrap().execute_batch("INSERT INTO contact_label VALUES(1,'Shared North',1),(2,'shared north',2),(3,'Shared South',3),(4,'',4)").unwrap();
    let tags = load_tags(&path, &HashMap::new()).unwrap();
    for name in ["SHARED NORTH", "shared"] {
        assert!(select_tag(&tags, name)
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
    }
    assert_eq!(select_tag(&tags, "").unwrap().name, "");
    assert!(select_tag(&tags, "absent").is_err());
}

#[test]
fn tags_bad_late_row_rejects_prior_partial_associations_without_secret_echo() {
    let (_temp, path) = contact_db();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("INSERT INTO contact_label VALUES(1,'one',1)")
        .unwrap();
    c.execute("INSERT INTO contact VALUES('alice',?1)", [labels("1")])
        .unwrap();
    c.execute("INSERT INTO contact VALUES('bob',?1)", [SECRET])
        .unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let err = load_tags(&path, &HashMap::new()).unwrap_err();
    assert!(!format!("{err:#}").contains(SECRET));
    assert_eq!(before, fs::read(&path).unwrap());
}

#[test]
fn tags_large_buffer_and_duplicate_fanout_are_rejected_by_latest_budgets() {
    let (_temp, path) = contact_db();
    let c = Connection::open(&path).unwrap();
    c.execute_batch("INSERT INTO contact_label VALUES(1,'one',1)")
        .unwrap();
    let mut oversized = labels("1");
    oversized.resize(2 * 1024 * 1024, 0);
    c.execute("INSERT INTO contact VALUES('alice',?1)", [&oversized])
        .unwrap();
    let duplicates = labels(&"1,".repeat(10_000));
    let user = "u".repeat(1024);
    c.execute(
        "INSERT INTO contact VALUES(?1,?2)",
        params![user, duplicates],
    )
    .unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let error = load_tags(&path, &HashMap::new()).unwrap_err();
    assert_eq!(error, contact_domain::Error::Limit);
    assert_eq!(before, fs::read(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM contact WHERE username='alice'", [])
        .unwrap();
    let before = fs::read(&path).unwrap();
    let error = load_tags(&path, &HashMap::new()).unwrap_err();
    assert_eq!(error, contact_domain::Error::Limit);
    assert_eq!(before, fs::read(&path).unwrap());
    println!("GUARD: 2MiB BLOB and 20KB duplicate-field fanout are independently rejected by latest byte budgets");
}

#[tokio::test]
async fn tags_oversized_query_precedes_cache_access_and_count_caps_are_explicit() {
    let (_temp, path) = contact_db();
    let db = DbCache::new(path.parent().unwrap().into());
    let error = contacts::q_tag_members(&db, &HashMap::new(), &" ".repeat(4097))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("query byte limit"));
    assert!(db.requests.lock().unwrap().is_empty());
    let c = Connection::open(&path).unwrap();
    c.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10001) INSERT INTO contact_label SELECT 1,'duplicate',x FROM n").unwrap();
    let error = load_tags(&path, &HashMap::new()).unwrap_err();
    assert_eq!(error, contact_domain::Error::Limit);
    c.execute_batch("DELETE FROM contact_label; INSERT INTO contact_label VALUES(1,'one',1)")
        .unwrap();
    c.execute(
        "INSERT INTO contact VALUES('u',?1)",
        [labels(&"1,".repeat(100001))],
    )
    .unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let error = load_tags(&path, &HashMap::new()).unwrap_err();
    assert_eq!(error, contact_domain::Error::Limit);
    assert_eq!(before, fs::read(&path).unwrap());
}

#[test]
fn tags_buffer_limit_is_inclusive_and_large_integer_ids_are_not_float_aliased() {
    let (_temp, path) = contact_db();
    let c = Connection::open(&path).unwrap();
    c.execute(
        "INSERT INTO contact_label VALUES(?1,'max-int',1)",
        [i64::MAX],
    )
    .unwrap();
    c.execute(
        "INSERT INTO contact_label VALUES(?1,'too-large-real',2)",
        [9_223_372_036_854_775_808.0f64],
    )
    .unwrap();
    let mut buffer = labels("9223372036854775807");
    buffer.resize(1_048_576, 0);
    c.execute("INSERT INTO contact VALUES('alice',?1)", [&buffer])
        .unwrap();
    let before = fs::read(&path).unwrap();
    let tags = load_tags(&path, &HashMap::new()).unwrap();
    assert_eq!(select_tag(&tags, "max-int").unwrap().members.len(), 1);
    assert_eq!(
        select_tag(&tags, "too-large-real").unwrap().members.len(),
        0
    );
    assert_eq!(before, fs::read(&path).unwrap());
    buffer.push(0);
    c.execute("UPDATE contact SET extra_buffer=?1", [&buffer])
        .unwrap();
    drop(c);
    let before = fs::read(&path).unwrap();
    let error = load_tags(&path, &HashMap::new()).unwrap_err();
    assert_eq!(error, contact_domain::Error::Limit);
    assert_eq!(before, fs::read(&path).unwrap());
}
