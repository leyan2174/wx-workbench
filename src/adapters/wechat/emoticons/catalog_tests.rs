use super::*;
use crate::daemon::cache::CacheMode;
use rusqlite::params;
use serde_json::{json, Value};
use std::fs;

const SCHEMA: &str = include_str!("../../../../tests/fixtures/emoticons-catalog/schema.sql");

fn database(path: &Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute_batch(sql).unwrap();
}

fn value(catalog: &Catalog) -> Value {
    let items: Vec<_> = catalog
        .items
        .iter()
        .map(|emoji| {
            let info = &emoji.info;
            let mut object = json!({"cdn_url": info.cdn_url, "aes_key": info.aes_key,
            "encrypt_url": info.encrypt_url, "product_id": info.product_id});
            if let Some(caption) = &info.caption {
                object["caption"] = json!(caption);
            }
            json!({"md5": emoji.md5, "info": object})
        })
        .collect();
    json!({"items": items, "non_store_count": catalog.non_store_count, "store_added": catalog.store_added})
}

#[test]
fn mapping_order_deduplication_templates_and_captions_are_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.db");
    database(
        &path,
        "INSERT INTO kNonStoreEmoticonTable VALUES
        ('seed','old','https://cdn.invalid/old?m=abc&x=1','old-encrypted','pkg'),
        ('seed','new','https://cdn.invalid/new?m=def&x=2',NULL,'pkg'),
        (NULL,NULL,'https://cdn.invalid/template?m=123&x=3',NULL,'pkg');
        INSERT INTO kStoreEmoticonFilesTable VALUES
        ('pkg','seed'),('pkg','added'),('pkg','added'),('missing','ignored');
        INSERT INTO kStoreEmoticonCaptionsTable VALUES
        ('seed','first','default'),('seed','second','default'),
        ('seed','ignored-language','en'),('added','store-caption','default');",
    );
    let before = fs::read(&path).unwrap();
    assert_eq!(
        value(&load_from_path(&path).unwrap()),
        json!({
            "items": [
                {"md5":"seed","info":{"cdn_url":"https://cdn.invalid/new?m=def&x=2","aes_key":"new","encrypt_url":"","product_id":"pkg","caption":"second"}},
                {"md5":"added","info":{"cdn_url":"https://cdn.invalid/template?m=added&x=3","aes_key":"","encrypt_url":"","product_id":"pkg","caption":"store-caption"}}
            ],
            "non_store_count": 1,
            "store_added": 1
        })
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn replacement_escapes_and_whole_match_are_explicit() {
    let temp = tempfile::tempdir().unwrap();
    for (i, (md5, expected)) in [
        ("plain", "m=plain&m=plain"),
        (r"a\nb", "m=a\nb&m=a\nb"),
        (r"a\\b", r"m=a\b&m=a\b"),
        (r"\g<0>", "m=m=abc&m=m=def"),
        (r"\&", r"m=\&&m=\&"),
        ("$1", "m=$1&m=$1"),
    ]
    .iter()
    .enumerate()
    {
        let path = temp.path().join(format!("escape-{i}.db"));
        database(
            &path,
            "INSERT INTO kNonStoreEmoticonTable VALUES(NULL,NULL,'m=abc&m=def&x=1',NULL,'p');",
        );
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO kStoreEmoticonFilesTable VALUES('p',?)", [md5])
            .unwrap();
        let catalog = load_from_path(&path).unwrap();
        assert_eq!(catalog.items.len(), 1);
        assert_eq!(catalog.items[0].info.cdn_url, format!("{expected}&x=1"));
    }
}

#[test]
fn errors_do_not_expose_schema_embedded_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private-schema.db");
    database(&path, "DROP TABLE kStoreEmoticonCaptionsTable; CREATE VIEW kStoreEmoticonCaptionsTable AS SELECT * FROM \"https://private.invalid/key-sentinel\";");
    let error = load_from_path(&path).err().unwrap();
    for message in [
        error.to_string(),
        format!("{error:#}"),
        format!("{error:?}"),
    ] {
        assert!(!message.contains("private.invalid"));
        assert!(!message.contains("key-sentinel"));
    }
}

#[test]
fn invalid_replacements_fail_even_without_a_match() {
    let temp = tempfile::tempdir().unwrap();
    for (i, md5) in [
        "trailing\\",
        r"\q",
        r"\1",
        r"\400",
        r"\g<1>",
        r"\g<bad>",
        r"\g<0",
    ]
    .iter()
    .enumerate()
    {
        for (j, template) in ["m=abc&x=1", "no-match&x=1"].iter().enumerate() {
            let path = temp.path().join(format!("invalid-{i}-{j}.db"));
            database(&path, "");
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO kNonStoreEmoticonTable VALUES(NULL,NULL,?,NULL,'p')",
                [template],
            )
            .unwrap();
            conn.execute("INSERT INTO kStoreEmoticonFilesTable VALUES('p',?)", [md5])
                .unwrap();
            drop(conn);
            assert!(load_from_path(&path).is_err(), "case {i}/{j}");
        }
    }
}

#[tokio::test]
async fn invalid_cache_key_errors_are_redacted_and_source_is_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source/emoticon/emoticon.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"synthetic-source").unwrap();
    let sentinel = "key-sentinel";
    let cache = cache_at(
        temp.path(),
        HashMap::from([("emoticon/emoticon.db".into(), sentinel.into())]),
    )
    .await;
    let error = load(&cache).await.err().unwrap();
    assert!(!format!("{error:#} {error:?}").contains(sentinel));
    assert_eq!(fs::read(&source).unwrap(), b"synthetic-source");
}

#[test]
fn schema_row_and_caption_faults_are_not_partial_success() {
    let cases = [
        "DROP TABLE kNonStoreEmoticonTable;",
        "DROP TABLE kStoreEmoticonFilesTable;",
        "ALTER TABLE kNonStoreEmoticonTable RENAME COLUMN aes_key TO bad;",
        "ALTER TABLE kStoreEmoticonCaptionsTable RENAME COLUMN caption_ TO bad;",
        "DROP TABLE kStoreEmoticonCaptionsTable; CREATE VIEW kStoreEmoticonCaptionsTable AS SELECT * FROM absent;",
        "INSERT INTO kNonStoreEmoticonTable VALUES('a',x'ff',NULL,NULL,NULL);",
        "INSERT INTO kNonStoreEmoticonTable VALUES(123,NULL,NULL,NULL,NULL);",
        "INSERT INTO kStoreEmoticonFilesTable VALUES('p',x'ff');",
        "INSERT INTO kStoreEmoticonCaptionsTable VALUES('a',x'ff','default');",
        "INSERT INTO kStoreEmoticonCaptionsTable VALUES(x'ff','caption','default');",
    ];
    let temp = tempfile::tempdir().unwrap();
    for (i, sql) in cases.iter().enumerate() {
        let path = temp.path().join(format!("bad-{i}.db"));
        database(
            &path,
            "INSERT INTO kNonStoreEmoticonTable VALUES('good',NULL,NULL,NULL,NULL);",
        );
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        assert!(load_from_path(&path).is_err(), "case {i}");
    }
    let missing = temp.path().join("missing.db");
    assert!(load_from_path(&missing).is_err());
    assert!(!missing.exists());
    let corrupt = temp.path().join("corrupt.db");
    fs::write(&corrupt, b"not sqlite").unwrap();
    assert!(load_from_path(&corrupt).is_err());
}

#[test]
fn locked_database_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("locked.db");
    database(&path, "");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    assert!(load_from_path(&path).is_err());
}

async fn cache_at(root: &Path, keys: HashMap<String, String>) -> DbCache {
    DbCache::with_dirs(
        root.join("source"),
        root.join("cache"),
        root.join("cache/_mtimes.json"),
        keys,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn exact_key_missing_source_and_canonical_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let keys = [
        "emoticon.db",
        "other/emoticon.db",
        "emoticon/emoticon.db-wal",
        "emoticon/../emoticon.db",
        "./emoticon/emoticon.db",
    ];
    let cache = cache_at(
        temp.path(),
        keys.into_iter()
            .map(|s| (s.into(), "invalid".into()))
            .collect(),
    )
    .await;
    assert!(load(&cache).await.unwrap().items.is_empty());
    let cache = cache_at(
        temp.path(),
        HashMap::from([("emoticon/emoticon.db".into(), "invalid".into())]),
    )
    .await;
    assert!(load(&cache).await.unwrap().items.is_empty());
    let cache = cache_at(
        temp.path(),
        HashMap::from([
            ("emoticon/emoticon.db".into(), "secret-sentinel".into()),
            ("EMOTICON\\EMOTICON.DB".into(), "secret-sentinel".into()),
        ]),
    )
    .await;
    let error = load(&cache).await.err().unwrap().to_string();
    assert!(error.contains("重名"));
    assert!(!error.contains("secret-sentinel"));
}

// 只在测试中加密合成页；生产解密和 WAL 应用始终调用仓库 crypto。
fn encrypted_pages(path: &Path, caption: &str, key: &[u8; 32], wal: bool) -> Vec<u8> {
    use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
    use hmac::{Hmac, Mac};
    use sha2::Sha512;
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA page_size=4096;").unwrap();
    let mut reserve: i32 = 80;
    // SAFETY: 连接在调用期间有效，参数是 SQLite 规定的可写 int 指针。
    let rc = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            std::ptr::null(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut i32).cast(),
        )
    };
    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute(
        "INSERT INTO kNonStoreEmoticonTable VALUES('a',NULL,NULL,NULL,NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kStoreEmoticonCaptionsTable VALUES('a',?,'default')",
        params![caption],
    )
    .unwrap();
    drop(conn);
    let plain = fs::read(path).unwrap();
    assert_eq!(plain[20], 80);
    let salt = [0x35u8; 16];
    let mut mac_key = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(key, &salt.map(|byte| byte ^ 0x3a), 2, &mut mac_key);
    let mut result = Vec::new();
    for (i, page) in plain.chunks_exact(4096).enumerate() {
        let start = if i == 0 && !wal { 16 } else { 0 };
        let iv = [0x24; 16];
        let enc = cbc::Encryptor::<aes::Aes256>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<NoPadding>(&page[start..4016]);
        let mut encrypted = vec![0u8; 4096];
        if start == 16 {
            encrypted[..16].copy_from_slice(&salt);
        }
        encrypted[start..4016].copy_from_slice(&enc);
        encrypted[4016..4032].copy_from_slice(&iv);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&encrypted[start..4032]);
        mac.update(&((i + 1) as u32).to_le_bytes());
        encrypted[4032..].copy_from_slice(&mac.finalize().into_bytes());
        result.extend(encrypted);
    }
    result
}

use crate::crypto::test_support::wal_bytes;

#[tokio::test]
async fn real_cache_cold_wal_incremental_restart_and_account_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let key = [0x42; 32];
    let raw = "EMOTICON\\EMOTICON.DB";
    let keys = HashMap::from([(raw.into(), "42".repeat(32))]);
    let a = temp.path().join("account-a");
    let b = temp.path().join("account-b");
    for account in [&a, &b] {
        fs::create_dir_all(account.join("source/EMOTICON")).unwrap();
    }
    let source = a.join("source/EMOTICON/EMOTICON.DB");
    let base = encrypted_pages(&temp.path().join("base.db"), "base", &key, false);
    fs::write(&source, &base).unwrap();
    let other_base = encrypted_pages(
        &temp.path().join("other.db"),
        "other-account",
        &[0x43; 32],
        false,
    );
    fs::write(b.join("source/EMOTICON/EMOTICON.DB"), &other_base).unwrap();
    let wal = source.with_file_name("EMOTICON.DB-wal");
    let first = encrypted_pages(&temp.path().join("first.db"), "cold-wal", &key, true);
    fs::write(&wal, wal_bytes(&first)).unwrap();
    let cache = cache_at(&a, keys.clone()).await;
    assert_eq!(
        load(&cache).await.unwrap().items[0].info.caption.as_deref(),
        Some("cold-wal")
    );
    let hit = cache.get_with_mode(raw).await.unwrap().unwrap();
    assert_eq!(hit.mode, CacheMode::CacheHit);
    assert_eq!(fs::read(&source).unwrap(), base);
    assert_eq!(fs::read(&wal).unwrap(), wal_bytes(&first));
    assert!(hit.path.starts_with(a.join("cache")));
    let cached_before = fs::metadata(&hit.path).unwrap().modified().unwrap();
    load(&cache).await.unwrap();
    assert_eq!(
        fs::metadata(&hit.path).unwrap().modified().unwrap(),
        cached_before
    );
    // 源库必须保持可认证；缓存模式直接证明 WAL 更新未走全量解密。
    std::thread::sleep(std::time::Duration::from_millis(30));
    let second = encrypted_pages(&temp.path().join("second.db"), "incremental", &key, true);
    fs::write(&wal, wal_bytes(&second)).unwrap();
    assert_eq!(
        cache.get_with_mode(raw).await.unwrap().unwrap().mode,
        CacheMode::WalIncremental
    );
    assert_eq!(
        load(&cache).await.unwrap().items[0].info.caption.as_deref(),
        Some("incremental")
    );
    drop(cache);
    std::thread::sleep(std::time::Duration::from_millis(30));
    let third = encrypted_pages(&temp.path().join("third.db"), "restart-wal", &key, true);
    fs::write(&wal, wal_bytes(&third)).unwrap();
    let restarted = cache_at(&a, keys.clone()).await;
    assert_eq!(
        restarted.get_with_mode(raw).await.unwrap().unwrap().mode,
        CacheMode::WalIncremental
    );
    assert_eq!(
        load(&restarted).await.unwrap().items[0]
            .info
            .caption
            .as_deref(),
        Some("restart-wal")
    );
    let other = cache_at(&b, HashMap::from([(raw.into(), "43".repeat(32))])).await;
    assert_eq!(
        load(&other).await.unwrap().items[0].info.caption.as_deref(),
        Some("other-account")
    );
    assert_ne!(other.get(raw).await.unwrap().unwrap(), hit.path);
    assert_eq!(fs::read(&source).unwrap(), base);
    assert_eq!(fs::read(&wal).unwrap(), wal_bytes(&third));
    assert_eq!(
        fs::read(b.join("source/EMOTICON/EMOTICON.DB")).unwrap(),
        other_base
    );
}
