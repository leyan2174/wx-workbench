use super::*;
use crate::daemon::query::encrypted_cache;
use std::{fs, path::PathBuf};

const CHAT: &str = "wxid_fixture";
const HASH: &str = "53fd943b058cad9eb3838c2ebbbdc55b";
const RESOURCE: &str = "message/message_resource.db";

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
    messages: Vec<PathBuf>,
    resource: PathBuf,
    dat: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db_dir = root.path().join("wxid_self/db_storage");
        let cache = root.path().join("cache");
        fs::create_dir_all(db_dir.join("message")).unwrap();
        fs::create_dir(&cache).unwrap();
        let message_keys = ["message/message_0.db", "message/message_1.db"];
        let mut keys = HashMap::new();
        let mut mtimes = serde_json::Map::new();
        let mut messages = Vec::new();
        let mut resource = PathBuf::new();
        for raw in message_keys.iter().copied().chain([RESOURCE]) {
            let source = db_dir.join(raw);
            let path = cache.join(format!("{:x}.db", md5::compute(raw)));
            let conn = encrypted_cache::sqlite(&path);
            if raw == RESOURCE {
                conn.execute_batch(
                    "CREATE TABLE ChatName2Id(user_name TEXT);
                    INSERT INTO ChatName2Id VALUES('wxid_fixture');
                    CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,
                    message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);",
                )
                .unwrap();
                resource = path.clone();
            } else {
                conn.execute_batch(&format!("CREATE TABLE Msg_{:x}(local_id INTEGER,local_type INTEGER,
                    create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER);",
                    md5::compute(CHAT))).unwrap();
                messages.push(path.clone());
            }
            drop(conn);
            let mt = encrypted_cache::seed(&path, &source);
            mtimes.insert(raw.into(), json!({"db_mt": mt, "wal_mt": 0, "path": path}));
            keys.insert(raw.into(), "11".repeat(32));
        }
        let mtimes_path = cache.join("_mtimes.json");
        fs::write(&mtimes_path, serde_json::to_vec(&mtimes).unwrap()).unwrap();
        let db = DbCache::with_dirs(db_dir.clone(), cache, mtimes_path, keys)
            .await
            .unwrap();
        let names = Names {
            map: HashMap::from([(CHAT.into(), "Fixture".into())]),
            md5_to_uname: HashMap::new(),
            msg_db_keys: message_keys.map(String::from).to_vec(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        };
        let dat = db_dir
            .parent()
            .unwrap()
            .join("msg/attach")
            .join(format!("{:x}", md5::compute(CHAT)))
            .join("2024-01/Img")
            .join(format!("{HASH}.dat"));
        fs::create_dir_all(dat.parent().unwrap()).unwrap();
        fs::write(&dat, b"invalid image bytes").unwrap();
        Self {
            _root: root,
            db,
            names,
            messages,
            resource,
            dat,
        }
    }

    fn insert(&self, shard: usize, id: i64, time: i64, kind: i64) {
        Connection::open(&self.messages[shard])
            .unwrap()
            .execute(
                &format!(
                    "INSERT INTO Msg_{:x} VALUES(?1,?2,?3,0,NULL,0)",
                    md5::compute(CHAT)
                ),
                rusqlite::params![id, kind, time],
            )
            .unwrap();
        let conn = Connection::open(&self.resource).unwrap();
        conn.execute(
            "INSERT INTO MessageResourceInfo VALUES(1,?1,?2,?3,?4)",
            rusqlite::params![id, kind, time, HASH.as_bytes()],
        )
        .unwrap();
    }

    async fn list(
        &self,
        enhanced: bool,
        limit: usize,
        offset: usize,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Value> {
        q_attachments_impl(
            &self.db,
            &self.names,
            CHAT,
            AttachmentQuery {
                kinds: None,
                page: MessagePage { limit, offset },
                since,
                until,
                meta: MetaOptions {
                    with_meta: false,
                    debug_source: false,
                },
            },
            enhanced,
        )
        .await
    }
}

#[tokio::test]
async fn production_global_page_raw_type_bounds_and_warm_repeat() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    f.insert(1, 2, 200, 3 | (1_i64 << 32));
    f.insert(0, 3, 300, 3);
    f.insert(1, 4, 400, 3);
    let before = fs::read(&f.resource).unwrap();
    for _ in 0..2 {
        let result = f.list(true, 1, 1, Some(100), Some(300)).await.unwrap();
        let rows = result["attachments"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["local_id"], 2);
        assert_eq!(rows[0]["md5"], HASH);
        assert_eq!(rows[0]["size"], fs::metadata(&f.dat).unwrap().len());
        assert_eq!(rows[0]["resource_status"], "found");
        assert_eq!(rows[0]["size_status"], "available");
    }
    assert_eq!(fs::read(&f.resource).unwrap(), before);
    let bounds = f.list(true, 20, 0, Some(100), Some(300)).await.unwrap();
    assert_eq!(
        bounds["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["local_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [3, 2, 1]
    );
}

#[tokio::test]
async fn production_default_and_empty_page_skip_invalid_resource() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    fs::write(&f.resource, b"broken resource").unwrap();
    // Keep the resource unavailable even when the cache attempts a rebuild.
    let source = f.db.db_dir().join(RESOURCE);
    let mut bytes = fs::read(&source).unwrap();
    bytes[4032] ^= 1;
    fs::write(source, bytes).unwrap();
    let ordinary = f.list(false, 20, 0, None, None).await.unwrap();
    assert_eq!(ordinary["count"], 1);
    assert!(ordinary["attachments"][0].get("md5").is_none());
    assert_eq!(f.list(true, 20, 10, None, None).await.unwrap()["count"], 0);
    assert!(f.list(true, 20, 0, None, None).await.is_err());
}

#[tokio::test]
async fn production_cross_shard_identity_is_not_arbitrarily_bound() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    f.insert(1, 1, 100, 3);
    let result = f.list(true, 1, 0, None, None).await.unwrap();
    let row = &result["attachments"][0];
    assert_eq!(row["resource_status"], "message_ambiguous");
    assert_eq!(row["size_status"], "not_requested");
    assert!(row["md5"].is_null() && row["size"].is_null());
}

#[tokio::test]
async fn production_size_zero_and_all_ranks_ambiguous() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    fs::write(&f.dat, []).unwrap();
    let result = f.list(true, 20, 0, None, None).await.unwrap();
    assert_eq!(result["attachments"][0]["size"], 0);
    fs::write(f.dat.with_file_name(format!("{HASH}_t.dat")), b"thumbnail").unwrap();
    let result = f.list(true, 20, 0, None, None).await.unwrap();
    assert_eq!(result["attachments"][0]["md5"], HASH);
    assert_eq!(result["attachments"][0]["size_status"], "ambiguous");
    assert!(result["attachments"][0]["size"].is_null());
}

#[tokio::test]
async fn production_same_shard_duplicate_is_message_ambiguous() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    f.insert(0, 1, 100, 3);
    let result = f.list(true, 1, 0, None, None).await.unwrap();
    assert_eq!(
        result["attachments"][0]["resource_status"],
        "message_ambiguous"
    );
    assert_eq!(result["attachments"][0]["size_status"], "not_requested");
    assert!(result["attachments"][0]["md5"].is_null());
}

#[tokio::test]
async fn production_duplicate_beyond_other_shard_timestamp_cap_is_found() {
    let f = Fixture::new().await;
    f.insert(0, 1, 100, 3);
    for id in 10..30 {
        f.insert(1, id, 100, 3);
    }
    f.insert(1, 1, 100, 3);
    let result = f.list(true, 1, 0, None, None).await.unwrap();
    assert_eq!(result["attachments"][0]["local_id"], 1);
    assert_eq!(
        result["attachments"][0]["resource_status"],
        "message_ambiguous"
    );
    Connection::open(&f.messages[1])
        .unwrap()
        .execute(
            &format!("DELETE FROM Msg_{:x} WHERE local_id=1", md5::compute(CHAT)),
            [],
        )
        .unwrap();
    // Remove the duplicate resource too: timestamp ties alone must not mark this unique row.
    Connection::open(&f.resource).unwrap().execute(
        "DELETE FROM MessageResourceInfo WHERE rowid=(SELECT max(rowid) FROM MessageResourceInfo WHERE message_local_id=1)", []
    ).unwrap();
    let result = f.list(true, 1, 0, None, None).await.unwrap();
    assert_eq!(result["attachments"][0]["resource_status"], "found");
    assert_eq!(result["attachments"][0]["md5"], HASH);
}

fn encrypted_sqlite(path: &std::path::Path) -> Vec<u8> {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("encrypted.db");
    encrypted_cache::seed(path, &output);
    fs::read(output).unwrap()
}

#[tokio::test]
async fn production_cold_cache_and_authorized_redecrypt_are_allowed() {
    for mode in ["cold", "redecrypt"] {
        let f = Fixture::new().await;
        f.insert(0, 1, 100, 3);
        let source = f.db.db_dir().to_owned();
        let cache = f._root.path().join(format!("{mode}-cache"));
        fs::create_dir(&cache).unwrap();
        let mtime = cache.join("_mtimes.json");
        let mut keys = HashMap::new();
        let mut persistent = serde_json::Map::new();
        let mut inputs = Vec::new();
        for (key, plain) in [
            ("message/message_0.db", &f.messages[0]),
            ("message/message_1.db", &f.messages[1]),
            (RESOURCE, &f.resource),
        ] {
            let encrypted = encrypted_sqlite(plain);
            let path = source.join(key);
            fs::write(&path, &encrypted).unwrap();
            let target = cache.join(format!("{:x}.db", md5::compute(key)));
            let stamp = fs::metadata(&path)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            if mode == "redecrypt" {
                fs::copy(plain, &target).unwrap();
            }
            persistent.insert(key.into(), json!({"db_mt":stamp,"wal_mt":0,"path":target}));
            keys.insert(key.into(), "11".repeat(32));
            inputs.push((path, encrypted));
        }
        if mode == "redecrypt" {
            fs::write(&mtime, serde_json::to_vec(&persistent).unwrap()).unwrap();
        }
        let db = DbCache::with_dirs(source, cache.clone(), mtime.clone(), keys)
            .await
            .unwrap();
        if mode == "redecrypt" {
            for (path, _) in &inputs {
                let modified = fs::metadata(path).unwrap().modified().unwrap()
                    + std::time::Duration::from_secs(2);
                fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
            for entry in persistent.values() {
                fs::write(
                    entry["path"].as_str().unwrap(),
                    b"stale cache must be rebuilt",
                )
                .unwrap();
            }
        }
        let dat = fs::read(&f.dat).unwrap();
        for _ in 0..2 {
            let result = q_attachments_with_image_metadata(
                &db,
                &f.names,
                CHAT,
                AttachmentQuery {
                    kinds: None,
                    page: MessagePage {
                        limit: 20,
                        offset: 0,
                    },
                    since: None,
                    until: None,
                    meta: MetaOptions {
                        with_meta: false,
                        debug_source: false,
                    },
                },
            )
            .await
            .unwrap();
            assert_eq!(result["attachments"][0]["md5"], HASH, "{mode}");
            assert_eq!(result["attachments"][0]["size"], dat.len() as u64, "{mode}");
        }
        assert!(mtime.exists());
        assert!(cache
            .join(format!("{:x}.db", md5::compute(RESOURCE)))
            .exists());
        for (path, bytes) in inputs {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(fs::read(&f.dat).unwrap(), dat);
    }
}
