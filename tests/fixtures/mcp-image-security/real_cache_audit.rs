use super::*;
use mcp_image_security::real_cache_query;

fn encrypted_sqlite(path: &Path) -> Vec<u8> {
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};
    let conn = Connection::open(path).unwrap();
    let mut reserve: std::ffi::c_int = 80;
    // Configure genuine SQLite pages with SQLCipher's reserved tail bytes.
    let status = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut std::ffi::c_int).cast(),
        )
    };
    assert_eq!(status, rusqlite::ffi::SQLITE_OK);
    conn.execute_batch("VACUUM").unwrap();
    let has_mapping:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE name='ChatName2Id'",[],|r|r.get(0)).unwrap();
    if has_mapping!=0 { conn.execute("UPDATE ChatName2Id SET rowid=7",[]).unwrap(); }
    drop(conn);
    let plain = fs::read(path).unwrap();
    assert_eq!(plain[20], 80);
    assert_eq!(plain.len() % 4096, 0);
    let mut output = Vec::new();
    for (index, page) in plain.chunks_exact(4096).enumerate() {
        let start = if index == 0 { 16 } else { 0 };
        let mut encrypted = vec![0u8; 4096];
        if index == 0 {
            encrypted[..16].fill(0x55);
        }
        let mut blocks: Vec<aes::cipher::Block<aes::Aes256>> = page[start..4016]
            .chunks_exact(16)
            .map(aes::cipher::Block::<aes::Aes256>::clone_from_slice)
            .collect();
        cbc::Encryptor::<aes::Aes256>::new((&[0x11; 32]).into(), (&[0x33; 16]).into())
            .encrypt_blocks_mut(&mut blocks);
        for (destination, block) in encrypted[start..4016].chunks_exact_mut(16).zip(blocks) {
            destination.copy_from_slice(&block);
        }
        encrypted[4016..4032].fill(0x33);
        output.extend(encrypted);
    }
    output
}

#[tokio::test]
async fn real_cache_cold_warm_and_redecrypt_first_exports_remain_usable() {
    for mode in ["cold", "warm", "redecrypt"] {
        let f = Account::new(b'A');
        let source = f.db.db_dir().to_owned();
        let cache_dir = f.message.parent().unwrap().to_owned();
        let mtime = cache_dir.join("_mtimes.json");
        let mut keys = std::collections::HashMap::new();
        let mut persistent = serde_json::Map::new();
        let mut sources = Vec::new();
        let mut cached = Vec::new();
        for (key, plain) in [
            ("message/message_0.db", &f.message),
            ("message/message_resource.db", &f.resource),
        ] {
            let encrypted = encrypted_sqlite(plain);
            let source_path = source.join(key);
            fs::write(&source_path, &encrypted).unwrap();
            let target = cache_dir.join(format!("{:x}.db", md5::compute(key)));
            let stamp = fs::metadata(&source_path)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            if mode != "cold" {
                fs::copy(plain, &target).unwrap();
            }
            persistent.insert(
                key.into(),
                serde_json::json!({"db_mt":stamp,"wal_mt":0,"path":target}),
            );
            keys.insert(key.into(), "11".repeat(32));
            sources.push((source_path, encrypted));
            cached.push(target);
        }
        if mode != "cold" {
            fs::write(&mtime, serde_json::to_vec(&persistent).unwrap()).unwrap();
        }
        let db = real_cache_query::cache(source, cache_dir.clone(), mtime.clone(), keys)
            .await
            .unwrap();
        if mode == "redecrypt" {
            for (path, _) in &sources {
                let modified = fs::metadata(path).unwrap().modified().unwrap()
                    + std::time::Duration::from_secs(2);
                fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
            for path in &cached {
                fs::write(path, b"stale cached bytes must be overwritten").unwrap();
            }
        }
        let out = real_cache_query::image::q_decode_image_with_key_file(
            &db, &f.names, CHAT, 42, 100, &f.output, None,
        )
        .await;
        println!("REAL CACHE {mode}: {out:?}");
        if out.is_err() {
            println!("REAL CACHE INNER DIAGNOSTIC: {:?}",real_cache_query::diagnostic(&db,&f.names,&f.output).await);
            for target in &cached {
                let conn=Connection::open(target).unwrap();
                println!("DECRYPTED INTEGRITY {}: {:?}",target.display(),conn.query_row("PRAGMA integrity_check",[],|r|r.get::<_,String>(0)));
            }
        }
        assert_eq!(out.unwrap()["status"], "published", "{mode}");
        assert_eq!(fs::read(f.destination()).unwrap(), f.plain);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&mtime).expect("mtime persistence must be writable"))
                .unwrap();
        assert_eq!(saved.as_object().unwrap().len(), 2);
        for (key, index) in [
            ("message/message_0.db", 0),
            ("message/message_resource.db", 1),
        ] {
            let resolved = db.get_with_mode(key).await.unwrap().unwrap();
            assert_eq!(resolved.mode.as_str(), "cache_hit");
            assert_eq!(resolved.path, cached[index]);
            assert!(fs::read(&resolved.path)
                .unwrap()
                .starts_with(b"SQLite format 3\0"));
            let current = fs::metadata(&sources[index].0)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            assert_eq!(
                saved[key]["db_mt"], current,
                "{mode}: persistence cannot silently fail"
            );
        }
        for (path, bytes) in sources {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
