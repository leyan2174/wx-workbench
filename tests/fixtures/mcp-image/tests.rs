use super::*;
use crate::daemon::query::encrypted_cache;
use rusqlite::{params, Connection};
use std::collections::HashMap;

const CHAT: &str = "wxid_image_peer";
const HASH: &str = "0123456789abcdef0123456789abcdef";
const PLAIN: &[u8] = b"\xff\xd8\xffsynthetic image\xff\xd9";

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
    messages: Vec<PathBuf>,
    resource: PathBuf,
    output: PathBuf,
    dat: PathBuf,
}

impl Fixture {
    async fn new(resource_keys: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let db_dir = root.path().join("wxid_self/db_storage");
        let cache = root.path().join("cache");
        let output = root.path().join("output");
        fs::create_dir_all(db_dir.join("message")).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&output).unwrap();
        let message_keys = ["message/message_0.db", "message/message_1.db"];
        let mut keys = HashMap::new();
        let mut mtimes = serde_json::Map::new();
        let mut messages = Vec::new();
        let mut resource = PathBuf::new();
        for raw in message_keys.iter().chain(resource_keys.iter()) {
            let source = db_dir.join(raw.replace('\\', "/"));
            let path = cache.join(format!("{:x}.db", md5::compute(raw)));
            let conn = encrypted_cache::sqlite(&path);
            if normalize(raw) == RESOURCE_KEY {
                conn.execute_batch(
                    "CREATE TABLE ChatName2Id(user_name TEXT);
                    CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,
                    message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO ChatName2Id(rowid,user_name) VALUES(9,?1)",
                    [CHAT],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO MessageResourceInfo VALUES(9,42,3,1700000000,?1)",
                    [HASH.as_bytes()],
                )
                .unwrap();
                resource = path.clone();
            } else {
                conn.execute_batch(&format!(
                    "CREATE TABLE Msg_{:x}(local_id INTEGER,local_type INTEGER,
                    create_time INTEGER,WCDB_CT_message_content,message_content)",
                    md5::compute(CHAT)
                ))
                .unwrap();
                messages.push(path.clone());
            }
            drop(conn);
            let mt = encrypted_cache::seed(&path, &source);
            mtimes.insert(raw.to_string(), json!({"db_mt":mt,"wal_mt":0,"path":path}));
            keys.insert(raw.to_string(), "11".repeat(32));
        }
        let mtime_file = cache.join("_mtimes.json");
        fs::write(&mtime_file, serde_json::to_vec(&mtimes).unwrap()).unwrap();
        let db = DbCache::with_dirs(db_dir.clone(), cache, mtime_file, keys)
            .await
            .unwrap();
        let names = Names {
            map: HashMap::from([(CHAT.into(), "Image Peer".into())]),
            md5_to_uname: HashMap::new(),
            msg_db_keys: message_keys.map(String::from).to_vec(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        };
        let img = db_dir
            .parent()
            .unwrap()
            .join("msg/attach")
            .join(format!("{:x}", md5::compute(CHAT)))
            .join("2023-11/Img");
        fs::create_dir_all(&img).unwrap();
        let dat = img.join(format!("{HASH}.dat"));
        fs::write(&dat, PLAIN.iter().map(|b| b ^ 0xa5).collect::<Vec<_>>()).unwrap();
        let f = Self {
            _root: root,
            db,
            names,
            messages,
            resource,
            output,
            dat,
        };
        f.insert(0, 42, 1700000000, 3);
        f
    }

    fn insert(&self, shard: usize, id: i64, time: i64, kind: i64) {
        Connection::open(&self.messages[shard])
            .unwrap()
            .execute(
                &format!(
                    "INSERT INTO Msg_{:x} VALUES(?1,?2,?3,0,NULL)",
                    md5::compute(CHAT)
                ),
                params![id, kind, time],
            )
            .unwrap();
    }
    async fn query(&self, time: i64) -> Result<Value> {
        q_decode_image(
            &self.db,
            &self.names,
            CHAT,
            42,
            time,
            &self.output,
            V2KeyMaterial::default(),
        )
        .await
    }
    fn empty_output(&self) {
        assert_eq!(fs::read_dir(&self.output).unwrap().count(), 0);
    }
    async fn fails(&self, text: &str) {
        let dat = fs::read(&self.dat).unwrap();
        let e = self.query(0).await.unwrap_err();
        assert!(format!("{e:#}").contains(text), "expected {text}: {e:#}");
        self.empty_output();
        assert_eq!(fs::read(&self.dat).unwrap(), dat);
    }
}

#[tokio::test]
async fn exact_current_account_exports_and_reports_full_identity() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    let db_before = fs::read(&f.resource).unwrap();
    let dat_before = fs::read(&f.dat).unwrap();
    let out = f.query(0).await.unwrap();
    assert_eq!(out["exit_code"], 0);
    assert_eq!(out["status"], "published");
    assert_eq!(out["image"]["message"]["source"], "message/message_0.db");
    assert_eq!(out["image"]["message"]["local_type"], 3);
    assert_eq!(out["image"]["message"]["create_time"], 1700000000);
    assert_eq!(
        fs::read(out["image"]["path"].as_str().unwrap()).unwrap(),
        PLAIN
    );
    assert_eq!(fs::read(&f.resource).unwrap(), db_before);
    assert_eq!(fs::read(&f.dat).unwrap(), dat_before);
    assert!(f.query(0).await.is_err());
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[tokio::test]
async fn preserves_raw_uppercase_backslash_resource_key() {
    let raw = "MESSAGE\\MESSAGE_RESOURCE.DB";
    let f = Fixture::new(&[raw]).await;
    assert!(f.db.get(RESOURCE_KEY).await.unwrap().is_none());
    assert_eq!(
        f.db.raw_db_keys(),
        vec![raw, "message/message_0.db", "message/message_1.db"]
    );
    assert_eq!(f.query(0).await.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn filters_full_key_inventory_without_selecting_similar_names() {
    let similar = "message/message_resource.db.bak";
    let f = Fixture::new(&[RESOURCE_KEY, similar]).await;
    assert_eq!(f.db.raw_db_keys().len(), 4);
    assert_eq!(f.query(0).await.unwrap()["exit_code"], 0);
    Fixture::new(&[similar])
        .await
        .fails("resource key must be present and unique")
        .await;
}

#[tokio::test]
async fn missing_key_duplicate_aliases_and_missing_source_fail_before_output() {
    Fixture::new(&[])
        .await
        .fails("resource key must be present and unique")
        .await;
    Fixture::new(&[RESOURCE_KEY, "MESSAGE\\MESSAGE_RESOURCE.DB"])
        .await
        .fails("resource key must be present and unique")
        .await;
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    fs::remove_file(f.db.db_dir().join(RESOURCE_KEY)).unwrap();
    f.fails("resource source must be present and unique").await;
}

#[tokio::test]
async fn unknown_resource_or_message_shards_refuse_publication() {
    for extra in [
        "MESSAGE_9.DB",
        "MESSAGE_0_RESOURCE.DB",
        "message_resource_1.db",
    ] {
        let f = Fixture::new(&[RESOURCE_KEY]).await;
        fs::write(f.db.db_dir().join("message").join(extra), b"unknown").unwrap();
        f.fails(if extra == "MESSAGE_9.DB" {
            "unknown message shards"
        } else {
            "unknown resource database"
        })
        .await;
    }
}

#[tokio::test]
async fn resource_directory_and_unavailable_message_shard_fail() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    let source = f.db.db_dir().join(RESOURCE_KEY);
    fs::remove_file(&source).unwrap();
    fs::create_dir(source).unwrap();
    f.fails("inventory source must be a regular file").await;
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    fs::remove_file(f.db.db_dir().join("message/message_1.db")).unwrap();
    f.fails("unavailable message shard").await;
}

#[tokio::test]
async fn uniqueness_precedes_type_filter_and_zero_time_uses_actual_time() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    f.insert(1, 42, 1700000001, 1);
    let out = f.query(0).await.unwrap();
    assert_eq!(out["exit_code"], 2);
    f.empty_output();
    assert_eq!(f.query(1700000000).await.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn nonimage_and_missing_ambiguous_chats_have_no_side_effects() {
    let mut f = Fixture::new(&[]).await;
    Connection::open(&f.messages[0])
        .unwrap()
        .execute_batch(&format!(
            "UPDATE Msg_{:x} SET local_type=49",
            md5::compute(CHAT)
        ))
        .unwrap();
    assert_eq!(
        f.query(0).await.unwrap()["text"],
        "expected image base_type=3"
    );
    f.names.map.insert("wxid_other".into(), "Image Peer".into());
    for (chat, code) in [("missing", 1), ("Image Peer", 2)] {
        let out = q_decode_image(
            &f.db,
            &f.names,
            chat,
            42,
            0,
            &f.output,
            V2KeyMaterial::default(),
        )
        .await
        .unwrap();
        assert_eq!(out["exit_code"], code);
    }
    f.empty_output();
}

#[tokio::test]
async fn full_type_flags_are_forwarded_without_masked_resource_fallback() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    Connection::open(&f.messages[0])
        .unwrap()
        .execute_batch(&format!(
            "UPDATE Msg_{:x} SET local_type=4294967299",
            md5::compute(CHAT)
        ))
        .unwrap();
    f.fails("exact image resource not found").await;
    Connection::open(&f.resource)
        .unwrap()
        .execute_batch("UPDATE MessageResourceInfo SET message_local_type=4294967299")
        .unwrap();
    assert_eq!(
        f.query(0).await.unwrap()["image"]["message"]["local_type"],
        4294967299i64
    );
}

#[tokio::test]
async fn mismatched_resource_time_and_duplicate_resource_records_fail() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    Connection::open(&f.resource)
        .unwrap()
        .execute_batch("UPDATE MessageResourceInfo SET message_create_time=1700000001")
        .unwrap();
    f.fails("exact image resource not found").await;
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    Connection::open(&f.resource)
        .unwrap()
        .execute_batch("INSERT INTO MessageResourceInfo SELECT * FROM MessageResourceInfo")
        .unwrap();
    f.fails("ambiguous exact image resource").await;
}

#[tokio::test]
async fn inventory_changes_are_detected_before_export_call() {
    for mutation in 0..4 {
        let f = Fixture::new(&[RESOURCE_KEY]).await;
        let before = Inventory::capture(f.db.db_dir(), &f.names.msg_db_keys, RESOURCE_KEY).unwrap();
        match mutation {
            0 => fs::write(f.db.db_dir().join("message/message_7.db"), b"new").unwrap(),
            1 => fs::write(
                f.db.db_dir().join("message/message_resource.db-wal"),
                b"new",
            )
            .unwrap(),
            2 => fs::write(f.db.db_dir().join(RESOURCE_KEY), b"changed resource size").unwrap(),
            _ => fs::remove_file(f.db.db_dir().join("message/message_1.db")).unwrap(),
        }
        assert!(before
            .verify(f.db.db_dir(), &f.names.msg_db_keys, RESOURCE_KEY)
            .is_err());
        f.empty_output();
    }
}

#[tokio::test]
async fn explicit_v2_key_is_required_and_forwarded() {
    use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    let aes = b"1234567890abcdef";
    let mut block = [13u8; 16];
    block[..3].copy_from_slice(&PLAIN[..3]);
    let mut block = GenericArray::clone_from_slice(&block);
    aes::Aes128::new(aes.into()).encrypt_block(&mut block);
    let mut dat = crate::attachment::decoder::V2_MAGIC.to_vec();
    dat.extend_from_slice(&3u32.to_le_bytes());
    dat.extend_from_slice(&2u32.to_le_bytes());
    dat.push(0);
    dat.extend_from_slice(&block);
    dat.extend_from_slice(&PLAIN[3..PLAIN.len() - 2]);
    dat.extend(PLAIN[PLAIN.len() - 2..].iter().map(|b| b ^ 0xa2));
    fs::write(&f.dat, dat).unwrap();
    f.fails("AES key").await;
    let missing = q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, &f.output, None)
        .await
        .unwrap_err();
    assert_eq!(
        format!("{missing:#}"),
        "image decoding failed; V2 requires an explicit valid image key"
    );
    let key_file = f._root.path().join("image-key.json");
    assert!(
        q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, &f.output, Some(&key_file))
            .await
            .is_err()
    );
    let out = q_decode_image_with_material(
        &f.db,
        &f.names,
        CHAT,
        42,
        0,
        &f.output,
        V2KeyMaterial {
            aes_key: Some(b"1234567890abcdef"),
            xor_key: 0xa2,
        },
    )
    .await
    .unwrap();
    assert_eq!(out["image"]["decoder"], "v2");
    assert_eq!(
        fs::read(out["image"]["path"].as_str().unwrap()).unwrap(),
        PLAIN
    );
}

#[tokio::test]
async fn native_root_policy_and_resource_corruption_remain_fail_closed() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    assert!(q_decode_image(
        &f.db,
        &f.names,
        CHAT,
        42,
        0,
        f.dat.parent().unwrap(),
        V2KeyMaterial::default()
    )
    .await
    .is_err());
    f.empty_output();
    fs::write(&f.resource, b"not sqlite").unwrap();
    // Corrupt the source too, so cache recovery cannot repair this failure fixture.
    let source = f.db.db_dir().join(RESOURCE_KEY);
    let mut bytes = fs::read(&source).unwrap();
    bytes[4032] ^= 1;
    fs::write(source, bytes).unwrap();
    assert!(f.query(0).await.is_err());
    f.empty_output();
}

#[tokio::test]
async fn plaintext_key_override_is_refused_before_account_and_file_access() {
    let f = Fixture::new(&[]).await;
    for key in &f.names.msg_db_keys {
        fs::remove_file(f.db.db_dir().join(key)).unwrap();
    }
    let missing_key = f._root.path().join("SECRET-missing-image-key.json");
    for output in [Path::new(""), Path::new("relative-output")] {
        let error =
            q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, output, Some(&missing_key))
                .await
                .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "Legacy plaintext image key files are unsupported"
        );
    }
    f.empty_output();
}

#[tokio::test]
async fn host_wrapper_exports_legacy_and_v1_with_default_key_material() {
    for v1 in [false, true] {
        let f = Fixture::new(&[RESOURCE_KEY]).await;
        if v1 {
            use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
            let mut block = [13u8; 16];
            block[..3].copy_from_slice(&PLAIN[..3]);
            let mut block = GenericArray::clone_from_slice(&block);
            aes::Aes128::new(b"cfcd208495d565ef".into()).encrypt_block(&mut block);
            let mut dat = crate::attachment::decoder::V1_MAGIC.to_vec();
            dat.extend_from_slice(&3u32.to_le_bytes());
            dat.extend_from_slice(&2u32.to_le_bytes());
            dat.push(0);
            dat.extend_from_slice(&block);
            dat.extend_from_slice(&PLAIN[3..PLAIN.len() - 2]);
            dat.extend(PLAIN[PLAIN.len() - 2..].iter().map(|b| b ^ 0x88));
            fs::write(&f.dat, dat).unwrap();
        }
        let out = q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, &f.output, None)
            .await
            .unwrap();
        assert_eq!(out["status"], "published");
        assert_eq!(
            out["image"]["decoder"],
            if v1 { "v1_aes" } else { "legacy_xor" }
        );
        assert_eq!(
            fs::read(out["image"]["path"].as_str().unwrap()).unwrap(),
            PLAIN
        );
    }
}

#[tokio::test]
async fn host_wrapper_isolates_source_cache_and_explicit_key_file() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    for output in [f.db.db_dir(), f.resource.parent().unwrap()] {
        let error = q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, output, None)
            .await
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "image output conflicts with protected input"
        );
    }
    f.empty_output();
    let key = f.output.join("image-key.json");
    fs::write(&key, b"SECRET protected sentinel").unwrap();
    let error = q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, &f.output, Some(&key))
        .await
        .unwrap_err();
    assert_eq!(
        format!("{error:#}"),
        "Legacy plaintext image key files are unsupported"
    );
    assert_eq!(fs::read(key).unwrap(), b"SECRET protected sentinel");
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[tokio::test]
async fn host_wrapper_key_file_limits_and_errors_are_redacted() {
    let f = Fixture::new(&[RESOURCE_KEY]).await;
    let path = f._root.path().join("SECRET-image-key.json");
    for bytes in [vec![b'S'; 4097], b"SECRET invalid json".to_vec()] {
        fs::write(&path, bytes).unwrap();
        let error =
            q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 0, &f.output, Some(&path))
                .await
                .unwrap_err();
        let text = format!("{error:#}");
        assert!(!text.contains("SECRET"));
        assert!(!text.contains("image-key.json"));
        f.empty_output();
    }
    assert!(q_decode_image_with_key_file(
        &f.db,
        &f.names,
        CHAT,
        42,
        0,
        &f.output,
        Some(Path::new("relative-key.json"))
    )
    .await
    .is_err());
    f.empty_output();
}
