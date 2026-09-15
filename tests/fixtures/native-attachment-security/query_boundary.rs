// 复用既有合成缓存容器；SQLite、严格定位、清单校验和附件处理均为真实源。
use mcp_readonly_security_harness::{DbCache, Names};
use rusqlite::{params, types::Value as SqlValue, Connection};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[path = "../../../src/daemon/query/mcp_attachments.rs"]
mod attachments;
#[path = "../../../src/daemon/query/chat_identity.rs"]
#[allow(dead_code)] // This harness exercises attachment reads, not export projections.
mod chat_identity;
#[path = "../../../src/daemon/query/strict_message.rs"]
#[allow(dead_code)] // Attachment queries do not consume every strict-message getter.
mod strict_message;

// 与 query.rs 的私有 glue 同形，完整性判断仍委托真实 meta helper。
fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> anyhow::Result<()> {
    let unknown = mcp_readonly_security_harness::meta::discover_unknown_shards_checked(
        db.db_dir(),
        &names.msg_db_keys,
    )?;
    anyhow::ensure!(
        unknown.is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}

const HASH: &str = "900150983cd24fb0d6963f7d28e17f72";
const SECRET: &str = "SYNTHETIC_CDN_KEY_DO_NOT_ECHO";
fn body() -> String {
    format!("<msg><appmsg><type>6</type><title>report.txt</title><appattach><totallen>3</totallen><cdnattachurl>{SECRET}</cdnattachurl></appattach><md5>{HASH}</md5></appmsg></msg>")
}
fn table() -> String {
    format!("Msg_{:x}", md5::compute("peer"))
}
fn create(path: &Path) {
    Connection::open(path).unwrap().execute_batch(&format!("CREATE TABLE [{}](local_id INTEGER,local_type,create_time,WCDB_CT_message_content,message_content)", table())).unwrap();
}
struct Account {
    temp: tempfile::TempDir,
    db: DbCache,
    names: Names,
    paths: Vec<PathBuf>,
}
impl Account {
    fn new(shards: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("account/db_storage");
        fs::create_dir_all(root.join("message")).unwrap();
        let mut db = DbCache::new(root);
        let mut names = Names::default();
        names.map.insert("peer".into(), "Peer".into());
        let mut paths = Vec::new();
        for index in 0..shards {
            let key = format!("message/message_{index}.db");
            let path = db.root.join(&key);
            create(&path);
            db.paths.insert(key.clone(), path.clone());
            names.msg_db_keys.push(key);
            paths.push(path);
        }
        Self {
            temp,
            db,
            names,
            paths,
        }
    }
    fn insert(&self, shard: usize, kind: i64, content: SqlValue, compressed: i64) {
        Connection::open(&self.paths[shard])
            .unwrap()
            .execute(
                &format!("INSERT INTO [{}] VALUES(7,?1,100,?2,?3)", table()),
                params![kind, compressed, content],
            )
            .unwrap();
    }
    fn file(&self, bytes: &[u8]) -> PathBuf {
        let path = self.db.root.parent().unwrap().join("msg/file/report.txt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    async fn query(&self, index: Option<i64>) -> anyhow::Result<Value> {
        attachments::q_attachment_reference(&self.db, &self.names, "peer", 7, 100, index).await
    }
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    walk(&entry.path(), output);
                } else {
                    output.insert(entry.path(), fs::read(entry.path()).unwrap());
                }
            }
        }
        let mut result = BTreeMap::new();
        walk(self.temp.path(), &mut result);
        result
    }
}

#[tokio::test]
async fn sql_ambiguity_precedes_type_and_record_index() {
    let f = Account::new(2);
    f.insert(0, 49, SqlValue::Text(body()), 0);
    f.insert(1, 1, SqlValue::Text("not attachment".into()), 0);
    let before = f.snapshot();
    for index in [None, Some(0), Some(999)] {
        let value = f.query(index).await.unwrap();
        assert_eq!(value["exit_code"], 2, "{value}");
        assert!(value["text"]
            .as_str()
            .unwrap()
            .contains("ambiguous message"));
        assert!(value.get("reference").is_none());
    }
    assert_eq!(f.snapshot(), before);
}

#[tokio::test]
async fn typed_attachment_selection_preserves_high_type_bits_and_rejects_other_formats() {
    for kind in [49, (6_i64 << 32) | 49, 3, 1, -1] {
        let f = Account::new(1);
        f.insert(0, kind, SqlValue::Text(body()), 0);
        f.file(b"abc");
        let before = f.snapshot();
        let value = f.query(None).await.unwrap();
        if kind == 49 || kind == ((6_i64 << 32) | 49) {
            assert_eq!(value["exit_code"], 0, "{value}");
            assert_eq!(value["status"], "found");
            assert_eq!(value["metadata"]["identity"]["local_id"], 7);
        } else {
            assert_eq!(value["exit_code"], 1, "{value}");
            assert_eq!(value["text"], "expected app message base_type=49");
            assert!(value.get("reference").is_none());
        }
        assert_eq!(f.snapshot(), before);
    }
}

#[tokio::test]
async fn missing_unloaded_and_invalid_inventory_never_become_missing_attachment() {
    for mode in ["missing", "unloaded", "corrupt", "unknown", "late"] {
        let mut f = Account::new(2);
        f.insert(
            0,
            49,
            SqlValue::Text("<msg><appmsg><type>19</type></appmsg></msg>".into()),
            0,
        );
        match mode {
            "missing" => {
                fs::remove_file(&f.paths[1]).unwrap();
            }
            "unloaded" => {
                f.db.paths.remove("message/message_1.db");
            }
            "corrupt" => {
                fs::write(&f.paths[1], b"synthetic invalid SQLite").unwrap();
            }
            "unknown" => create(&f.db.root.join("message/MESSAGE_9.DB")),
            _ => {
                let staged = f.temp.path().join("staged.db");
                create(&staged);
                *f.db.publish_on_get.lock().unwrap() =
                    Some((1, staged, f.db.root.join("message/message_9.db")));
            }
        }
        let before = f.snapshot();
        let result = f.query(Some(999)).await;
        assert!(
            result.is_err(),
            "完整性错误不能降级为 NotLoaded/missing: {mode} {result:?}"
        );
        assert!(!format!("{:?}", result).contains(SECRET));
        if mode != "late" {
            assert_eq!(f.snapshot(), before);
        } else {
            for (path, bytes) in before {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
        }
    }
}

#[tokio::test]
async fn query_root_is_bound_to_selected_cache_account() {
    let a = Account::new(1);
    let b = Account::new(1);
    for f in [&a, &b] {
        f.insert(0, 49, SqlValue::Text(body()), 0);
    }
    let path = a.file(b"abc");
    b.file(b"bad");
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    let found = a.query(None).await.unwrap();
    let rejected = b.query(None).await.unwrap();
    assert_eq!(found["status"], "found");
    assert_eq!(
        Path::new(found["reference"]["path"].as_str().unwrap()),
        path
    );
    assert_eq!(rejected["exit_code"], 1);
    assert!(rejected["text"].as_str().unwrap().contains("HashMismatch"));
    assert!(!found.to_string().contains(SECRET));
    assert!(!rejected.to_string().contains(SECRET));
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[tokio::test]
async fn ordinary_and_record_decoded_limits_and_stored_limit_are_not_swallowed() {
    for (size, index, compressed, expected) in [
        (20_001, None, false, "decoded byte limit"),
        (20_001, None, true, "decoded byte limit"),
        (500_001, Some(0), true, "decoded byte limit"),
        (1_048_577, None, false, "stored byte limit"),
    ] {
        let f = Account::new(1);
        let bytes = vec![b'x'; size];
        let value = if compressed {
            SqlValue::Blob(zstd::encode_all(bytes.as_slice(), 1).unwrap())
        } else {
            SqlValue::Text(String::from_utf8(bytes).unwrap())
        };
        f.insert(0, 49, value, if compressed { 4 } else { 0 });
        let before = f.snapshot();
        let error = f.query(index).await.unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
        assert_eq!(f.snapshot(), before);
    }
}

#[tokio::test]
async fn sql_nested_hash_must_not_publish_wrong_heuristic_reference() {
    let f = Account::new(1);
    f.file(b"bad");
    f.insert(
        0,
        49,
        SqlValue::Text(body().replace(
            &format!("<md5>{HASH}</md5>"),
            &format!("<md5><value>{HASH}</value></md5>"),
        )),
        0,
    );
    let before = f.snapshot();
    let value = f.query(None).await.unwrap();
    println!("SQL NESTED HASH RESPONSE: {value}");
    assert_eq!(f.snapshot(), before);
    assert_ne!(
        value["exit_code"], 0,
        "不能把结构错误的 MD5 当成未声明后成功返回错误附件: {value}"
    );
}
