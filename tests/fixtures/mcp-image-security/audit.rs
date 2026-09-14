use mcp_image_security::{attachment::decoder::V2KeyMaterial, mcp_image, DbCache, Names};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const HASH: &str = "0123456789abcdef0123456789abcdef";
const CHAT: &str = "synthetic_peer";
mod real_cache_audit;
mod publish_probe_audit;

#[tokio::test]
async fn guard_directory_swap_must_not_publish_into_previously_protected_root() {
    use std::sync::Arc;
    let mut f=Account::new(b'A');
    let protected=f.root.path().join("protected-decrypted");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("sensitive.db"),b"synthetic protected bytes").unwrap();
    f.db.2.push(protected.clone());
    let protected_id=same_file::Handle::from_path(&protected).unwrap();
    let f=Arc::new(f);
    let entered=Arc::new(tokio::sync::Notify::new()); let release=Arc::new(tokio::sync::Notify::new());
    *f.db.1.lock().unwrap()=Some((entered.clone(),release.clone()));
    let task_f=f.clone();
    let task=tokio::spawn(async move { mcp_image::q_decode_image_with_key_file(&task_f.db,&task_f.names,CHAT,42,100,&task_f.output,None).await });
    tokio::time::timeout(std::time::Duration::from_secs(5),entered.notified()).await.unwrap();
    fs::rename(&f.output,f.root.path().join("original-output")).expect("reproduce actual output directory rename");
    fs::rename(&protected,&f.output).expect("reproduce actual protected directory relocation");
    assert_eq!(protected_id,same_file::Handle::from_path(&f.output).unwrap());
    release.notify_one();
    let result=task.await.unwrap();
    println!("GUARD SWAP result={result:?} published={} protected_identity_preserved=true",f.destination().exists());
    assert_eq!(fs::read(f.output.join("sensitive.db")).unwrap(),b"synthetic protected bytes");
    assert!(result.is_err(),"output directory identity changed to protected input during await; publication must fail closed");
    assert!(!f.destination().exists());
}

#[tokio::test]
async fn key_json_type_duplicate_null_and_exact_size_contract() {
    let rejected = [
        "null",
        "[]",
        "true",
        "0",
        "\"SYNTHETIC_SECRET\"",
        r#"{"image_aes_key":"SYNTHETIC_SECRET"}"#,
        r#"{"aes_key":123}"#,
        r#"{"aes_key":[]}"#,
        r#"{"aes_key":{}}"#,
        r#"{"aes_key":null,"aes_key":"1234567890abcdef"}"#,
        r#"{"xor_key":null,"xor_key":165}"#,
        r#"{"xor_key":-1}"#,
        r#"{"xor_key":256}"#,
        r#"{"xor_key":1.5}"#,
        r#"{"xor_key":true}"#,
        r#"{"xor_key":[]}"#,
        r#"{"xor_key":{}}"#,
        r#"{"xor_key":"0x100"}"#,
        r#"{"xor_key":" 165 "}"#,
        r#"{"aes_key":"short"}"#,
        "{\"aes_key\":\"1234567890abcde\\u00e9\"}",
        "{} {}",
        "\u{feff}{}",
    ];
    for json in rejected {
        let f = Account::new(b'A');
        let key = f.root.path().join("json-key.json");
        fs::write(&key, json).unwrap();
        let result = mcp_image::q_decode_image_with_key_file(
            &f.db,
            &f.names,
            CHAT,
            42,
            100,
            &f.output,
            Some(&key),
        )
        .await;
        let error = format!("{:#}", result.expect_err(json));
        assert!(!error.contains("SYNTHETIC_SECRET"));
        assert!(f.db.requests.lock().unwrap().is_empty(), "{json}");
        f.empty();
    }
    for json in [
        "{}",
        r#"{"aes_key":null,"xor_key":null}"#,
        r#"{"xor_key":165}"#,
        r#"{"xor_key":"0XA5"}"#,
        r#"{"aes_key":"1234567890abcdef_suffix"}"#,
    ] {
        let f = Account::new(b'A');
        let key = f.root.path().join("json-key.json");
        let mut bytes = json.as_bytes().to_vec();
        bytes.resize(4096, b' ');
        fs::write(&key, &bytes).unwrap();
        let result = mcp_image::q_decode_image_with_key_file(
            &f.db,
            &f.names,
            CHAT,
            42,
            100,
            &f.output,
            Some(&key),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "published");
        assert_eq!(fs::read(key).unwrap(), bytes);
    }
}

#[tokio::test]
async fn guard_locks_survive_query_await_and_release_on_all_exit_paths() {
    use std::sync::Arc;
    for mode in ["success", "decode-error", "cancel"] {
        let f = Arc::new(Account::new(b'A'));
        let key_dir = f.root.path().join("keys");
        fs::create_dir(&key_dir).unwrap();
        let key = key_dir.join("explicit.json");
        fs::write(&key, b"{}").unwrap();
        let alias = f.root.path().join("key-hardlink.json");
        fs::hard_link(&key, &alias).unwrap();
        if mode == "decode-error" {
            fs::write(&f.dat, b"not an image").unwrap();
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *f.db.1.lock().unwrap() = Some((entered.clone(), release.clone()));
        let task_f = f.clone();
        let task_key = key.clone();
        let task = tokio::spawn(async move {
            mcp_image::q_decode_image_with_key_file(
                &task_f.db,
                &task_f.names,
                CHAT,
                42,
                100,
                &task_f.output,
                Some(&task_key),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("wrapper must reach guarded await");
        for path in [&key, &alias] {
            assert!(
                fs::OpenOptions::new().write(true).open(path).is_err(),
                "{mode}: key write unlocked"
            );
        }
        assert!(fs::remove_file(&key).is_err(),"{mode}: original key deletion unlocked");
        let unlink_alias=fs::remove_file(&alias);
        println!("GUARD {mode}: hardlink alias unlink={unlink_alias:?}; original remains pinned={}",key.exists());
        assert_eq!(fs::read(&key).unwrap(),b"{}");
        assert!(fs::rename(&key_dir, f.root.path().join("moved-keys")).is_err());
        let moved=f.root.path().join("moved-output");
        let rename=fs::rename(&f.output,&moved);
        println!("GUARD {mode}: output rename while held={rename:?}");
        if rename.is_ok() { fs::rename(&moved,&f.output).unwrap(); }
        if mode == "cancel" {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            release.notify_one();
            let result = task.await.unwrap();
            assert_eq!(result.is_ok(), mode == "success");
        }
        assert!(
            fs::OpenOptions::new().write(true).open(&key).is_ok(),
            "{mode}: leaked key handle"
        );
        fs::rename(&key_dir, f.root.path().join("released-keys")).unwrap();
        fs::rename(&f.output, f.root.path().join("released-output")).unwrap();
        println!(
            "GUARD lifecycle {mode}: key/hardlink/ancestor locked while awaiting, released on exit"
        );
    }
}

#[tokio::test]
async fn actual_ipc_reader_failure_after_publication_has_no_receipt_or_rollback() {
    let f = Account::new(b'A');
    for suffix in ["_h.dat","_t.dat"] {
        fs::copy(&f.dat,f.dat.with_file_name(format!("{HASH}{suffix}"))).unwrap();
    }
    let value =
        mcp_image::q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 100, &f.output, None)
            .await
            .unwrap();
    let wire = mcp_image_security::ipc::Response::ok(value)
        .to_json_line()
        .unwrap();
    println!("ACTUAL IPC serialized size: {}",wire.len());
    assert!(wire.len() > 1024, "use a genuine oversized image response");
    let result = mcp_image_security::ipc_reader::read(wire.as_bytes(), 1024).await;
    println!(
        "ACTUAL IPC READER bytes={} result={result:?} published={}",
        wire.len(),
        f.destination().exists()
    );
    assert!(result.is_err());
    assert_eq!(fs::read(f.destination()).unwrap(), f.plain);
    let retry =
        mcp_image::q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 100, &f.output, None)
            .await;
    assert!(retry.is_err());
}
fn frames(arguments: serde_json::Value) -> String {
    use serde_json::json;
    [json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"audit","version":"1"},"capabilities":{}}}),
     json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
     json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"decode_image","arguments":arguments}})]
    .iter().map(|v|format!("{v}\n")).collect()
}

#[test]
fn host_policy_schema_injection_and_secret_error_boundary() {
    use serde_json::json;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    for mode in ["unconfigured", "configured", "client-path", "secret-error"] {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("account.json");
        let capture = root.path().join("request.json");
        let key = root.path().join("explicit-key.json");
        let account = root.path().join("account");
        let output_root = root.path().join("output");
        for path in [account.join("db_storage"), account.join("decrypted"), output_root.clone()] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(account.join("keys.json"), b"{}").unwrap();
        fs::write(&config, serde_json::to_vec(&json!({
            "db_dir": account.join("db_storage"),
            "keys_file": account.join("keys.json"),
            "decrypted_dir": account.join("decrypted"),
            "wechat_process": "SyntheticNeverLaunched.exe"
        })).unwrap()).unwrap();
        fs::write(&key, br#"{"xor_key":165}"#).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_host-probe"));
        command
            .env("WX_CLI_CONFIG", &config)
            .env("AUDIT_CAPTURE_REQUEST", &capture)
            .env_remove("AUDIT_SECRET_ERROR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if mode != "unconfigured" {
            command
                .arg("--media-output-root")
                .arg(&output_root)
                .arg("--image-key-file")
                .arg(&key);
        }
        if mode == "secret-error" {
            command.env("AUDIT_SECRET_ERROR", "1");
        }
        let mut arguments = json!({"chat_name":CHAT,"local_id":42,"create_time":100});
        if mode == "client-path" {
            arguments["output_root"] = json!("C:/untrusted");
        }
        println!("COMMAND: {command:?}");
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(frames(arguments).as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        println!("MODE {mode}\nSTDOUT: {stdout}\nSTDERR: {stderr}");
        assert!(!stdout.contains("SYNTHETIC_SECRET"));
        assert!(!stderr.contains("SYNTHETIC_SECRET"));
        let reply: serde_json::Value =
            serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
        if matches!(mode, "unconfigured" | "client-path") {
            assert!(!capture.exists());
            assert!(!stderr.contains("AUDIT_RUNTIME_REACHED"));
            assert!(reply.get("error").is_some() || reply["result"]["isError"] == true);
        } else {
            assert!(
                capture.exists(),
                "decode_image must actually reach host transport: {stdout}"
            );
            let request: serde_json::Value =
                serde_json::from_slice(&fs::read(capture).unwrap()).unwrap();
            assert_eq!(request["cmd"], "decode_image");
            assert_eq!(request["chat"], CHAT);
            assert_eq!(request["output_root"], output_root.to_str().unwrap());
            assert_eq!(request["image_key_file"], key.to_str().unwrap());
            if mode == "secret-error" {
                assert_eq!(reply["result"]["isError"], true);
            }
        }
    }
}

#[test]
fn response_limit_records_real_publication_before_delivery_failure() {
    use mcp_image_security::{
        ipc::{Request, Response},
        protocol::Protocol,
    };
    let f = Account::new(b'A');
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut calls = 0;
    let mut protocol = Protocol::new(|request: Request| {
        assert!(matches!(request, Request::DecodeImage { .. }));
        calls += 1;
        Ok(Response::ok(runtime.block_on(f.query(&f.output)).unwrap()))
    });
    let mut output = Vec::new();
    let result = protocol.serve(
        std::io::Cursor::new(frames(
            serde_json::json!({"chat_name":CHAT,"local_id":42,"create_time":100}),
        )),
        &mut output,
        1024,
    );
    println!(
        "LIMIT RESULT: {result:?}\nSTDOUT: {}\nPUBLISHED: {}",
        String::from_utf8_lossy(&output),
        f.destination().exists()
    );
    assert_eq!(calls, 1, "test must execute actual image publication");
    assert!(f.destination().exists());
    assert_eq!(fs::read(f.destination()).unwrap(), f.plain);
    // This observation is intentionally not labelled rollback or success delivery.
    assert!(result.is_err(), "exercise an actual response-limit failure");
    assert_eq!(String::from_utf8_lossy(&output).lines().count(), 1);
    let retry = runtime.block_on(f.query(&f.output));
    println!("RETRY AFTER LOST RESPONSE: {retry:?}");
    assert!(retry.is_err());
}
struct Account {
    root: tempfile::TempDir,
    db: DbCache,
    names: Names,
    output: PathBuf,
    dat: PathBuf,
    resource: PathBuf,
    message: PathBuf,
    plain: Vec<u8>,
}

#[tokio::test]
async fn v2_explicit_correct_key_only_and_wrong_or_missing_key_never_publishes() {
    use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
    for mode in ["missing", "wrong", "correct"] {
        let f = Account::new(b'A');
        let mut block = [9u8; 16];
        block[..7].copy_from_slice(&f.plain);
        let mut block = GenericArray::clone_from_slice(&block);
        aes::Aes128::new(b"1234567890abcdef".into()).encrypt_block(&mut block);
        let mut dat = native_image_fixture::decoder::V2_MAGIC.to_vec();
        dat.extend_from_slice(&7u32.to_le_bytes());
        dat.extend_from_slice(&0u32.to_le_bytes());
        dat.push(0);
        dat.extend_from_slice(&block);
        fs::write(&f.dat, &dat).unwrap();
        let key = f.root.path().join("v2-key.json");
        fs::write(
            &key,
            if mode == "correct" {
                br#"{"aes_key":"1234567890abcdef"}"#.as_slice()
            } else {
                br#"{"aes_key":"SYNTHETIC_SECRET_WRONG_KEY"}"#.as_slice()
            },
        )
        .unwrap();
        let result = mcp_image::q_decode_image_with_key_file(
            &f.db,
            &f.names,
            CHAT,
            42,
            100,
            &f.output,
            if mode == "missing" { None } else { Some(&key) },
        )
        .await;
        if mode == "correct" {
            assert_eq!(result.unwrap()["status"], "published");
            assert_eq!(fs::read(f.destination()).unwrap(), f.plain);
        } else {
            let error = format!("{:#}", result.unwrap_err());
            assert!(!error.contains("SYNTHETIC_SECRET"));
            f.empty();
        }
        assert_eq!(fs::read(&f.dat).unwrap(), dat);
    }
}

#[tokio::test]
async fn key_file_unconfigured_output_does_not_read_cache_or_secret() {
    let f = Account::new(b'A');
    let result = mcp_image::q_decode_image_with_key_file(
        &f.db,
        &f.names,
        CHAT,
        42,
        100,
        Path::new(""),
        Some(Path::new("SECRET_NOT_A_REAL_KEY_FILE")),
    )
    .await;
    let error = result.unwrap_err().to_string();
    assert!(!error.contains("SECRET"));
    assert!(f.db.requests.lock().unwrap().is_empty());
    f.empty();
}

#[tokio::test]
async fn key_file_errors_are_bounded_redacted_and_precede_database_access() {
    for mode in [
        "relative",
        "inside-output",
        "malformed",
        "invalid-aes",
        "invalid-xor",
        "oversized",
    ] {
        let f = Account::new(b'A');
        let key = if mode == "inside-output" {
            f.output.join("secret.json")
        } else {
            f.root.path().join("secret.json")
        };
        let bytes = match mode {
            "invalid-aes" => br#"{"aes_key":"SECRET"}"#.to_vec(),
            "invalid-xor" => br#"{"xor_key":"SYNTHETIC_SECRET"}"#.to_vec(),
            "oversized" => vec![b'S'; 4097],
            "malformed" => b"SYNTHETIC_SECRET_NOT_JSON".to_vec(),
            _ => b"{}".to_vec(),
        };
        fs::write(&key, &bytes).unwrap();
        let path = if mode == "relative" {
            Path::new("relative-key.json")
        } else {
            &key
        };
        let result = mcp_image::q_decode_image_with_key_file(
            &f.db,
            &f.names,
            CHAT,
            42,
            100,
            &f.output,
            Some(path),
        )
        .await;
        let error = format!("{:#}", result.unwrap_err());
        println!("KEY MODE {mode}: {error}");
        assert!(!error.contains("SECRET") && !error.contains("secret.json"));
        assert!(f.db.requests.lock().unwrap().is_empty(), "{mode}");
        assert_eq!(fs::read(&key).unwrap(), bytes);
        assert!(!f.destination().exists());
    }
}

#[tokio::test]
async fn host_output_cannot_overlap_source_or_decrypted_directories() {
    let f = Account::new(b'A');
    let source = f.db.db_dir();
    let decrypted = f.message.parent().unwrap();
    for output in [
        source.to_owned(),
        source.join("child-output"),
        decrypted.to_owned(),
        decrypted.join("child-output"),
        f.root.path().to_owned(),
    ] {
        fs::create_dir_all(&output).unwrap();
        let result =
            mcp_image::q_decode_image_with_key_file(&f.db, &f.names, CHAT, 42, 100, &output, None)
                .await;
        assert!(result.is_err(), "{}: {result:?}", output.display());
        assert!(f.db.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn explicit_key_file_works_and_hardlink_destination_never_overwrites_key() {
    for existing in [false, true] {
        let f = Account::new(b'A');
        let key = f.root.path().join("key.json");
        let bytes = br#"{"xor_key":"0xa5"}"#;
        fs::write(&key, bytes).unwrap();
        if existing {
            fs::hard_link(&key, f.destination()).unwrap();
        }
        let result = mcp_image::q_decode_image_with_key_file(
            &f.db,
            &f.names,
            CHAT,
            42,
            100,
            &f.output,
            Some(&key),
        )
        .await;
        if existing {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap()["status"], "published");
        }
        assert_eq!(fs::read(&key).unwrap(), bytes);
        assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
    }
}
impl Account {
    fn new(marker: u8) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("account/db_storage");
        let decrypted = root.path().join("decrypted");
        let output = root.path().join("output");
        for p in [source.join("message"), decrypted.clone(), output.clone()] {
            fs::create_dir_all(p).unwrap();
        }
        let mut db = DbCache::new(source.clone());
        let mut names = Names::default();
        names.map.insert(CHAT.into(), "Peer".into());
        let message_key = "message/message_0.db";
        let resource_key = "message/message_resource.db";
        let message = decrypted.join("message.db");
        let resource = decrypted.join("resource.db");
        for (key, path) in [(message_key, &message), (resource_key, &resource)] {
            fs::write(source.join(key), b"synthetic encrypted source").unwrap();
            db.0.paths.insert(key.into(), path.clone());
        }
        names.msg_db_keys.push(message_key.into());
        Connection::open(&message).unwrap().execute_batch(&format!("CREATE TABLE Msg_{:x}(local_id INTEGER,local_type INTEGER,create_time INTEGER,WCDB_CT_message_content,message_content); INSERT INTO Msg_{:x} VALUES(42,3,100,0,NULL)",md5::compute(CHAT),md5::compute(CHAT))).unwrap();
        let conn = Connection::open(&resource).unwrap();
        conn.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id(rowid,user_name) VALUES(7,'synthetic_peer'); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB)").unwrap();
        conn.execute(
            "INSERT INTO MessageResourceInfo VALUES(7,42,3,100,?1)",
            [HASH.as_bytes()],
        )
        .unwrap();
        drop(conn);
        let dat = source.parent().unwrap().join(format!(
            "msg/attach/{:x}/2026-09/Img/{HASH}.dat",
            md5::compute(CHAT)
        ));
        fs::create_dir_all(dat.parent().unwrap()).unwrap();
        let plain = vec![0xff, 0xd8, 0xff, marker, marker, 0xff, 0xd9];
        fs::write(&dat, plain.iter().map(|v| v ^ 0xa5).collect::<Vec<_>>()).unwrap();
        Self {
            root,
            db,
            names,
            output,
            dat,
            resource,
            message,
            plain,
        }
    }
    async fn query(&self, output: &Path) -> anyhow::Result<serde_json::Value> {
        mcp_image::q_decode_image_with_key_file(
            &self.db,
            &self.names,
            CHAT,
            42,
            100,
            output,
            None,
        )
        .await
    }
    fn destination(&self) -> PathBuf {
        self.output
            .join(format!("{:x}.jpg", md5::compute(&self.plain)))
    }
    fn sources(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        [&self.dat, &self.resource, &self.message]
            .into_iter()
            .chain(
                self.names
                    .msg_db_keys
                    .iter()
                    .map(|k| self.db.root.join(k))
                    .collect::<Vec<_>>()
                    .iter(),
            )
            .map(|p| (p.clone(), fs::read(p).unwrap()))
            .collect()
    }
    fn empty(&self) {
        assert_eq!(fs::read_dir(&self.output).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn query_same_ids_different_accounts_stay_separate() {
    let a = Account::new(b'A');
    let b = Account::new(b'B');
    let before_a = a.sources();
    let before_b = b.sources();
    for f in [&a, &b] {
        let out = f.query(&f.output).await.unwrap();
        assert_eq!(out["status"], "published");
        let path = Path::new(out["image"]["path"].as_str().unwrap());
        assert!(path.starts_with(&f.output));
        assert_eq!(fs::read(path).unwrap(), f.plain);
        assert!(
            same_file::is_same_file(out["image"]["source_path"].as_str().unwrap(), &f.dat).unwrap()
        );
    }
    assert_eq!(a.sources(), before_a);
    assert_eq!(b.sources(), before_b);
}

#[tokio::test]
async fn invalid_output_paths_do_not_publish_or_mutate_sources() {
    let f = Account::new(b'A');
    let before = f.sources();
    for output in [
        PathBuf::from("relative"),
        f.output.join(".."),
        f.output.join("."),
        f.root.path().join("output:ads"),
        f.root.path().join("CON"),
        PathBuf::from(r"\\localhost\share"),
        f.dat.parent().unwrap().to_owned(),
    ] {
        assert!(f.query(&output).await.is_err(), "{}", output.display());
        f.empty();
        assert_eq!(f.sources(), before);
    }
}

#[tokio::test]
async fn existing_output_hardlinks_to_all_protected_sources_are_never_overwritten() {
    for which in 0..3 {
        let f = Account::new(b'A');
        let source = [&f.dat, &f.resource, &f.message][which];
        fs::hard_link(source, f.destination()).unwrap();
        let before = f.sources();
        assert!(f.query(&f.output).await.is_err());
        assert_eq!(f.sources(), before);
        assert_eq!(
            fs::read(f.destination()).unwrap(),
            fs::read(source).unwrap()
        );
        assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn repeated_publication_fails_without_overwrite_or_temp_leftover() {
    let f = Account::new(b'A');
    f.query(&f.output).await.unwrap();
    assert!(f.query(&f.output).await.is_err());
    assert_eq!(fs::read(f.destination()).unwrap(), f.plain);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[tokio::test]
async fn exact_duplicate_messages_fail_before_export() {
    let f = Account::new(b'A');
    Connection::open(&f.message)
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO Msg_{:x} VALUES(42,3,100,0,NULL)",
            md5::compute(CHAT)
        ))
        .unwrap();
    let out = f.query(&f.output).await.unwrap();
    assert_eq!(out["exit_code"], 2);
    f.empty();
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Windows symlink creation denied with OS error 1314; see baseline.log"]
async fn output_symlink_directory_is_rejected() {
    let f = Account::new(b'A');
    let link = f.root.path().join("output-link");
    std::os::windows::fs::symlink_dir(&f.output, &link)
        .expect("real symlink setup required; permission failure is not a pass");
    assert!(f.query(&link).await.is_err());
    f.empty();
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "Windows symlink creation denied with OS error 1314; see baseline.log"]
async fn source_dat_symlink_is_rejected() {
    let f = Account::new(b'A');
    let target = f.root.path().join("external.dat");
    fs::rename(&f.dat, &target).unwrap();
    std::os::windows::fs::symlink_file(&target, &f.dat)
        .expect("real symlink setup required; permission failure is not a pass");
    let before = fs::read(&target).unwrap();
    assert!(f.query(&f.output).await.is_err());
    f.empty();
    assert_eq!(fs::read(target).unwrap(), before);
}

#[cfg(windows)]
#[tokio::test]
async fn output_junction_is_rejected_without_external_publication() {
    use std::os::windows::process::CommandExt;
    let f = Account::new(b'A');
    let link = f.root.path().join("output-junction");
    let mut command = std::process::Command::new("cmd.exe");
    command
        .args(["/d", "/u", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&f.output)
        .creation_flags(0x08000000);
    println!("COMMAND: {command:?}");
    let result = command.output().unwrap();
    let decode = |bytes: &[u8]| {
        String::from_utf16_lossy(
            &bytes
                .chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
    };
    println!(
        "STDOUT: {}\nSTDERR: {}",
        decode(&result.stdout),
        decode(&result.stderr)
    );
    assert!(result.status.success());
    let before = f.sources();
    assert!(f.query(&link).await.is_err());
    f.empty();
    assert_eq!(f.sources(), before);
}
