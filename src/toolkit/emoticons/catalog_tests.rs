use super::*;
use crate::daemon::cache::CacheMode;
use rusqlite::params;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

const SCHEMA: &str = include_str!("../../../tests/fixtures/emoticons-catalog/schema.sql");

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

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("vendor/wechat-decrypt/emoticons.py").is_file())
        .unwrap()
        .to_owned()
}

#[test]
fn legacy_ast_golden_all_mapping_boundaries() {
    let cases = [
        "",
        "INSERT INTO kNonStoreEmoticonTable VALUES(NULL,NULL,NULL,NULL,NULL),('',NULL,'https://x?m=abc&x=1',NULL,'p'),('a',NULL,NULL,NULL,NULL); INSERT INTO kStoreEmoticonFilesTable VALUES('p','s'),('p',NULL),('p','');",
        "INSERT INTO kNonStoreEmoticonTable VALUES('b','old','https://x?m=abc&v=1','e','p'),('a',NULL,NULL,NULL,NULL),('b','new','https://x?m=def&v=2',NULL,'p'),(NULL,NULL,'https://last?m=abc&z=1',NULL,'p'),(NULL,NULL,'',NULL,'p'); INSERT INTO kStoreEmoticonFilesTable VALUES('p','b'),('p','s'),('p','s'),('missing','t');",
        "INSERT INTO kNonStoreEmoticonTable VALUES('a',NULL,NULL,NULL,NULL); INSERT INTO kStoreEmoticonCaptionsTable VALUES('a','first','default'),('a','ignore','en'),('a',NULL,'default'),('missing','no','default'),(NULL,'no','default'),('a','wrong-case','Default');",
        "DROP TABLE kStoreEmoticonCaptionsTable; INSERT INTO kNonStoreEmoticonTable VALUES('a',NULL,NULL,NULL,NULL);",
        "INSERT INTO kNonStoreEmoticonTable VALUES('UPPER',NULL,'https://x?m=abcd&x=1',NULL,'P'),('upper',NULL,NULL,NULL,NULL); INSERT INTO kStoreEmoticonFilesTable VALUES('p','no'),('P','UPPER'),('P','third'); INSERT INTO kStoreEmoticonCaptionsTable VALUES('third','yes','default');",
    ];
    let temp = tempfile::tempdir().unwrap();
    for (i, sql) in cases.iter().enumerate() {
        let path = temp.path().join(format!("case-{i}.db"));
        database(&path, sql);
        compare_legacy(&path);
    }
    // 包括没有 m、没有 &、大写、部分匹配、非参数边界和多次匹配。
    let urls = [
        "https://x?m=abc",
        "https://x?m=ABC&x=1",
        "https://x?M=abc&x=1",
        "https://x?a=1&b=2",
        "https://x?m=abCDEF&m=012&xm=af&x=1",
        "https://x?m=&x=1",
        "https://x/pathm=abc&",
        "&m=0g&m=fff",
        "&",
    ];
    for (i, url) in urls.iter().enumerate() {
        let path = temp.path().join(format!("url-{i}.db"));
        database(&path, "");
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO kNonStoreEmoticonTable VALUES(NULL,NULL,?,NULL,'p')",
            [url],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO kStoreEmoticonFilesTable VALUES('p','NEW'),('p','second');",
        )
        .unwrap();
        drop(conn);
        compare_legacy(&path);
    }
}

fn compare_legacy(path: &Path) {
    let before = fs::read(path).unwrap();
    let output =
        Command::new(std::env::var_os("WX_CATALOG_PYTHON").unwrap_or_else(|| "python".into()))
            .arg(root().join("tests/fixtures/emoticons-catalog/oracle.py"))
            .arg(root().join("vendor/wechat-decrypt/emoticons.py"))
            .arg(path)
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value(&load_from_path(path).unwrap()) == expected,
        "legacy mapping mismatch"
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn legacy_replacement_escapes_and_whole_match_reference() {
    let temp = tempfile::tempdir().unwrap();
    for (i, md5) in [r"a\nb", r"a\\b", r"\101", r"\0", r"\g<0>", r"\&", "$1"]
        .iter()
        .enumerate()
    {
        let path = temp.path().join(format!("escape-{i}.db"));
        database(
            &path,
            "INSERT INTO kNonStoreEmoticonTable VALUES(NULL,NULL,'m=abc&m=def',NULL,'p');",
        );
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO kStoreEmoticonFilesTable VALUES('p',?)", [md5])
            .unwrap();
        compare_legacy(&path);
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
            let output = Command::new(
                std::env::var_os("WX_CATALOG_PYTHON").unwrap_or_else(|| "python".into()),
            )
            .arg(root().join("tests/fixtures/emoticons-catalog/oracle.py"))
            .arg(root().join("vendor/wechat-decrypt/emoticons.py"))
            .arg(&path)
            .output()
            .unwrap();
            assert!(
                !output.status.success(),
                "legacy accepted invalid replacement case {i}/{j}"
            );
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
    let mut result = Vec::new();
    for (i, page) in plain.chunks_exact(4096).enumerate() {
        let start = if i == 0 && !wal { 16 } else { 0 };
        let iv = [0x24; 16];
        let enc = cbc::Encryptor::<aes::Aes256>::new(key.into(), (&iv).into())
            .encrypt_padded_vec_mut::<NoPadding>(&page[start..4016]);
        let mut encrypted = vec![0u8; 4096];
        encrypted[start..4016].copy_from_slice(&enc);
        encrypted[4016..4032].copy_from_slice(&iv);
        result.extend(encrypted);
    }
    result
}

fn wal_bytes(pages: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    bytes[..4].copy_from_slice(&0x377f0682u32.to_be_bytes());
    bytes[8..12].copy_from_slice(&4096u32.to_be_bytes());
    bytes[16..24].copy_from_slice(&[7; 8]);
    for (i, page) in pages.chunks_exact(4096).enumerate() {
        let mut header = [0u8; 24];
        header[..4].copy_from_slice(&((i + 1) as u32).to_be_bytes());
        header[4..8].copy_from_slice(&((pages.len() / 4096) as u32).to_be_bytes());
        header[8..16].copy_from_slice(&[7; 8]);
        bytes.extend(header);
        bytes.extend(page);
    }
    bytes
}

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
    // 保留主库 mtime 后破坏密文，证明后续成功不是偷偷重新全量解密。
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    fs::write(&source, vec![0u8; base.len()]).unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let second = encrypted_pages(&temp.path().join("second.db"), "incremental", &key, true);
    fs::write(&wal, wal_bytes(&second)).unwrap();
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
    assert_eq!(fs::read(&source).unwrap(), vec![0u8; base.len()]);
    assert_eq!(fs::read(&wal).unwrap(), wal_bytes(&third));
    assert_eq!(
        fs::read(b.join("source/EMOTICON/EMOTICON.DB")).unwrap(),
        other_base
    );
}
