use super::*;
use crate::daemon::{query::encrypted_cache, server::dispatch_state};
use crate::ipc::Request;
use serde_json::json;
use std::fs;

#[tokio::test]
async fn extraction_uses_bound_image_material_and_protected_publication() {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let root = tempfile::tempdir().unwrap();
    let runtime = super::tests::runtime(root.path());
    super::tests::seed(&runtime);
    let cached = runtime.cache_dir().join("resource.db");
    let conn = encrypted_cache::sqlite(&cached);
    conn.execute_batch(
        "CREATE TABLE ChatName2Id(user_name TEXT);
        INSERT INTO ChatName2Id VALUES('peer');
        CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,
        message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);",
    )
    .unwrap();
    let hash = "a".repeat(32);
    let mut packed = vec![0x12, 0x22, 0x0a, 0x20];
    packed.extend_from_slice(hash.as_bytes());
    conn.execute(
        "INSERT INTO MessageResourceInfo VALUES(1,7,3,1577836800,?1)",
        [packed],
    )
    .unwrap();
    drop(conn);
    let source = runtime.config.db_dir.join("message/message_resource.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let mt = encrypted_cache::seed(&cached, &source);
    let mut mtimes: serde_json::Value =
        serde_json::from_slice(&fs::read(runtime.mtime_file()).unwrap()).unwrap();
    mtimes["message/message_resource.db"] = json!({"db_mt":mt,"wal_mt":0,"path":cached});
    fs::write(runtime.mtime_file(), serde_json::to_vec(&mtimes).unwrap()).unwrap();
    let store = crate::key_store::Store::for_runtime(&runtime).unwrap();
    let mut keys = store.load().unwrap().database_keys();
    keys.insert("message/message_resource.db".into(), "11".repeat(32));
    store
        .update(
            None,
            &[crate::key_store::Update::Databases(
                &keys,
                crate::key_store::Verification::Verified,
            )],
        )
        .unwrap();

    let aes = [0x31; 16];
    let plain = b"\x89PNG\r\n\x1a\nsynthetic image";
    let padding = 16 - plain.len() % 16;
    let mut encrypted = plain.to_vec();
    encrypted.resize(plain.len() + padding, padding as u8);
    let cipher = aes::Aes128::new_from_slice(&aes).unwrap();
    for block in encrypted.chunks_exact_mut(16) {
        cipher.encrypt_block(block.into());
    }
    let mut dat = crate::attachment::decoder::V2_MAGIC.to_vec();
    dat.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    dat.extend_from_slice(&0u32.to_le_bytes());
    dat.push(0);
    dat.extend_from_slice(&encrypted);
    let dat_path = root
        .path()
        .join("msg/attach")
        .join(format!("{:x}", md5::compute("peer")))
        .join("2020-01/Img")
        .join(format!("{hash}.dat"));
    fs::create_dir_all(dat_path.parent().unwrap()).unwrap();
    fs::write(&dat_path, &dat).unwrap();
    let id = crate::attachment::AttachmentId {
        v: 1,
        chat: "peer".into(),
        local_id: 7,
        create_time: 1577836800,
        kind: crate::attachment::AttachmentKind::Image,
        db: None,
    }
    .encode()
    .unwrap();
    let output = root.path().join("out/image.png");
    let request = |path: &std::path::Path, overwrite| Request::Extract {
        attachment_id: id.clone(),
        output: path.to_string_lossy().into_owned(),
        overwrite,
    };
    let state = QueryState::new(runtime.clone());
    let missing = dispatch_state(request(&output, false), &state).await;
    assert!(!missing.ok);
    assert!(!output.parent().unwrap().exists());
    drop(state);
    store
        .update(
            None,
            &[crate::key_store::Update::Image(
                &aes,
                0x88,
                crate::key_store::Verification::Verified,
            )],
        )
        .unwrap();
    let state = QueryState::new(runtime.clone());
    let reply = dispatch_state(request(&output, false), &state).await;
    assert!(reply.ok, "{reply:?}");
    assert_eq!(fs::read(&output).unwrap(), plain);
    assert_eq!(reply.data["format"], "png");
    assert!(!dispatch_state(request(&output, false), &state).await.ok);
    fs::write(&output, b"previous output").unwrap();
    assert!(dispatch_state(request(&output, true), &state).await.ok);
    assert_eq!(fs::read(&output).unwrap(), plain);
    for protected in [&dat_path, &runtime.config_path, &source] {
        let before = fs::read(protected).unwrap();
        assert!(!dispatch_state(request(protected, true), &state).await.ok);
        assert_eq!(fs::read(protected).unwrap(), before);
    }
    let alias = root.path().join("alias.dat");
    fs::hard_link(&dat_path, &alias).unwrap();
    assert!(!dispatch_state(request(&alias, true), &state).await.ok);
    assert_eq!(fs::read(&dat_path).unwrap(), dat);
    fs::remove_file(alias).unwrap();
    fs::write(store.path(), b"synthetic damaged store").unwrap();
    assert!(dispatch_state(request(&output, true), &state).await.ok);
    assert_eq!(fs::read(&output).unwrap(), plain);
    fs::write(&dat_path, b"invalid DAT").unwrap();
    assert!(!dispatch_state(request(&output, true), &state).await.ok);
    assert_eq!(fs::read(&output).unwrap(), plain);
    drop(state);
    let cold = QueryState::new(runtime);
    assert!(!dispatch_state(request(&output, true), &cold).await.ok);
    assert_eq!(fs::read(&output).unwrap(), plain);
}
