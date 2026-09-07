//! delta/plan 过程级安全边界；全部资料现场合成，不发现真实账号。
use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256, Sha512};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const USER: &str = "../../escape' OR 1=1 --";
const BODY: &str = r#"{"username":"victim","source":"../../keys.json","msg_uid":"FORGED","timestamp":999,"local_id":999,"extras":{"source":"FORGED"}}"#;

struct Fixture {
    root: tempfile::TempDir,
    account: bool,
    protected: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
            account: false,
            protected: Vec::new(),
        };
        fs::create_dir(f.path("profile")).unwrap();
        fs::write(f.path("profile/config.json"), b"INVALID_AMBIENT_CONFIG").unwrap();
        f
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn remember(&mut self, path: PathBuf) {
        self.protected.push((path.clone(), fs::read(path).unwrap()));
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.protected {
            assert_eq!(
                &fs::read(path).unwrap(),
                bytes,
                "源资料被改写: {}",
                path.display()
            );
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        println!("COMMAND: {:?} {:?}", env!("CARGO_BIN_EXE_wx"), args);
        let output = Command::new(env!("CARGO_BIN_EXE_wx"))
            .args(args)
            .current_dir(self.root.path())
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_USERS")
            .env_remove("WECHAT_EXPORT_CONTACTS")
            .env("WX_CLI_CONFIG", self.path("profile/config.json"))
            .env("WX_CLI_HOME", self.path("runtime"))
            .env("WX_WECHAT_DECRYPT_PYTHON", self.path("absent-python.exe"))
            .env("PATH", "")
            .output()
            .unwrap();
        println!(
            "STDOUT:\n{}\nSTDERR:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for bytes in [&output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(bytes).contains(&"11".repeat(32)),
                "进程输出泄露合成密钥"
            );
        }
        output
    }
    fn account(&mut self, corrupt: bool) {
        self.account = true;
        let source = self.path("profile/db_storage");
        fs::create_dir_all(source.join("contact")).unwrap();
        fs::create_dir_all(source.join("message")).unwrap();
        let contact_plain = self.path("contact-plain.db");
        let db = reserved_database(&contact_plain);
        db.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,description TEXT,local_type INTEGER);").unwrap();
        db.execute(
            "INSERT INTO contact VALUES (?1,'same display','../malicious',0,?2,0)",
            params![USER, BODY],
        )
        .unwrap();
        db.execute(
            "INSERT INTO contact VALUES ('victim','same display','',0,'',0)",
            [],
        )
        .unwrap();
        drop(db);
        encrypt(&contact_plain, &source.join("contact/contact.db"));
        let message_plain = self.path("message-plain.db");
        let db = reserved_database(&message_plain);
        for username in [USER, "victim"] {
            let table = format!("Msg_{:x}", md5::compute(username));
            db.execute_batch(&format!("CREATE TABLE [{table}] (local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER);")).unwrap();
            let body = if username == USER {
                BODY
            } else {
                "FOREIGN_ACCOUNT_MESSAGE"
            };
            db.execute(
                &format!("INSERT INTO [{table}] VALUES (1,1,100,NULL,?1,0)"),
                [body],
            )
            .unwrap();
            if corrupt && username == USER {
                db.execute_batch(&format!(
                    "INSERT INTO [{table}] VALUES (2,1,101,NULL,123456789,0)"
                ))
                .unwrap();
            }
        }
        drop(db);
        encrypt(&message_plain, &source.join("message/message_0.db"));
        fs::write(self.path("profile/all_keys.json"), serde_json::json!({"contact/contact.db":"11".repeat(32),"message/message_0.db":"11".repeat(32)}).to_string()).unwrap();
        fs::write(
            self.path("profile/config.json"),
            br#"{"db_dir":"db_storage","keys_file":"all_keys.json","decrypted_dir":"decrypted"}"#,
        )
        .unwrap();
        for name in [
            "profile/db_storage/contact/contact.db",
            "profile/db_storage/message/message_0.db",
            "profile/all_keys.json",
            "profile/config.json",
        ] {
            self.remember(self.path(name));
        }
    }
    fn cache(&mut self) -> PathBuf {
        let cache = self.path("cache");
        fs::create_dir(&cache).unwrap();
        let path = cache.join("messages.db");
        let db = Connection::open(&path).unwrap();
        let table = format!("Msg_{:x}", md5::compute("alpha"));
        db.execute_batch(&format!("CREATE TABLE [{table}] (create_time INTEGER,message_content TEXT,compress_content BLOB,packed_info_data BLOB); INSERT INTO [{table}] VALUES (100,'SYNTHETIC_BODY_MUST_NOT_BE_EXPORTED',NULL,NULL);")).unwrap();
        drop(db);
        self.remember(path);
        cache
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.account {
            let _ = self.run(&["daemon", "stop"]);
        }
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn failed(output: Output) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "意外成功: {text}");
    text
}
fn value(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn delta_rejects_run_ids_and_protected_paths_without_writing() {
    let mut f = Fixture::new();
    f.account(false);
    let output = f.path("output");
    for run in [
        "../escape",
        "..\\escape",
        "C:stream",
        "CON",
        "run.",
        "run ",
        "run\ncontrol",
    ] {
        failed(f.run(&[
            "toolkit",
            "export-delta-native",
            arg(&output),
            "--users",
            USER,
            "--start",
            "100",
            "--run-id",
            run,
        ]));
        assert!(!output.exists());
    }
    for path in [
        f.path("profile/db_storage/new"),
        f.path("profile/all_keys.json"),
        f.path("profile/config.json"),
        f.path("profile/db_storage/../db_storage/escape"),
    ] {
        failed(f.run(&[
            "toolkit",
            "export-delta-native",
            arg(&path),
            "--users",
            USER,
            "--start",
            "100",
            "--run-id",
            "safe",
        ]));
    }
    failed(f.run(&[
        "toolkit",
        "export-delta-native",
        arg(&output),
        "--users",
        USER,
        "--start",
        "101",
        "--end",
        "100",
        "--run-id",
        "safe",
    ]));
    assert!(!output.exists());
    assert!(!f.path("profile/db_storage/new").exists());
    assert!(!f.path("profile/db_storage/escape").exists());
    assert!(!f.path("runtime").exists(), "输入校验不得触发账号后台");
    f.unchanged();
}

#[test]
fn plan_rejects_identity_pollution_and_database_traversal_without_output() {
    let mut f = Fixture::new();
    let cache = f.cache();
    let output = f.path("plan.csv");
    let metadata = f.path("chats.json");
    for body in [
        r#"[{"username":"alpha"},{"username":"alpha","chat_name":"other"}]"#,
        r#"[{"username":"alpha","username":"victim"}]"#,
        r#"[{"username":{"source":"SECRET_METADATA"}}]"#,
    ] {
        fs::write(&metadata, body).unwrap();
        failed(f.run(&[
            "toolkit",
            "chat-plan-native",
            "--decrypted-dir",
            arg(&cache),
            "--chats-json",
            arg(&metadata),
            "--output",
            arg(&output),
        ]));
        assert!(!output.exists());
        assert_eq!(fs::read(&metadata).unwrap(), body.as_bytes());
    }
    fs::write(&metadata, r#"[{"username":"alpha","chat_name":"victim"}]"#).unwrap();
    failed(f.run(&[
        "toolkit",
        "chat-plan-native",
        "--decrypted-dir",
        arg(&cache),
        "--chats-json",
        arg(&metadata),
        "--user",
        "victim",
        "--output",
        arg(&output),
    ]));
    for database in ["../outside.db", "messages.db:secret", "C:\\outside.db"] {
        failed(f.run(&[
            "toolkit",
            "chat-plan-native",
            "--decrypted-dir",
            arg(&cache),
            "--user",
            "alpha",
            "--message-db",
            database,
            "--output",
            arg(&output),
        ]));
        assert!(!output.exists());
    }
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
    assert!(!f.path("runtime").exists());
    f.unchanged();
}

#[test]
fn plan_refuses_source_and_existing_targets_without_exposing_body() {
    let mut f = Fixture::new();
    let cache = f.cache();
    let existing = f.path("existing.csv");
    fs::write(&existing, b"KEEP_PLAN").unwrap();
    for output in [
        existing.clone(),
        cache.join("new.csv"),
        cache.join("messages.db"),
        f.path("profile/config.json"),
        f.path("cache/../escape.csv"),
    ] {
        let error = failed(f.run(&[
            "toolkit",
            "chat-plan-native",
            "--decrypted-dir",
            arg(&cache),
            "--message-db",
            "messages.db",
            "--user",
            "alpha",
            "--output",
            arg(&output),
        ]));
        assert!(!error.contains("SYNTHETIC_BODY_MUST_NOT_BE_EXPORTED"));
    }
    assert_eq!(fs::read(&existing).unwrap(), b"KEEP_PLAN");
    assert_eq!(
        fs::read(f.path("profile/config.json")).unwrap(),
        b"INVALID_AMBIENT_CONFIG"
    );
    assert!(!cache.join("new.csv").exists());
    assert!(!f.path("escape.csv").exists());
    assert!(!f.path("runtime").exists());
    f.unchanged();
}

#[test]
fn delta_untrusted_metadata_and_content_cannot_forge_identity_or_paths() {
    let mut f = Fixture::new();
    f.account(false);
    let output = f.path("delta");
    let report = value(f.run(&[
        "toolkit",
        "export-delta-native",
        arg(&output),
        "--users",
        USER,
        "--start",
        "100",
        "--run-id",
        "security",
    ]));
    assert_eq!(report["success"], true);
    let manifest_path = report["manifest_path"].as_str().unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["username"], USER);
    let run = Path::new(manifest_path).parent().unwrap();
    let path = run.join(files[0]["path"].as_str().unwrap());
    assert!(path
        .canonicalize()
        .unwrap()
        .starts_with(run.canonicalize().unwrap()));
    let bytes = fs::read(&path).unwrap();
    let chat: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(chat["username"], USER);
    assert_eq!(chat["contact_memo"], BODY);
    assert_eq!(chat["messages"].as_array().unwrap().len(), 1);
    let message = &chat["messages"][0];
    assert_eq!(message["local_id"], 1);
    assert_eq!(message["timestamp"], 100);
    assert_eq!(message["source"], "message/message_0.db");
    assert!(message.get("username").is_none());
    let hash = format!("{:x}", Sha256::digest(BODY.as_bytes()));
    let uid = format!(
        "{:x}",
        Sha256::digest(format!("{USER}|message_0.db|1|100|text|{hash}").as_bytes())
    );
    assert_eq!(message["msg_uid"], uid);
    assert!(!String::from_utf8_lossy(&bytes).contains("FOREIGN_ACCOUNT_MESSAGE"));
    let manifest_before = fs::read(manifest_path).unwrap();
    failed(f.run(&[
        "toolkit",
        "export-delta-native",
        arg(&output),
        "--users",
        "victim",
        "--start",
        "100",
        "--run-id",
        "security",
    ]));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
    f.unchanged();
}

#[test]
fn delta_query_failure_never_publishes_partial_chat() {
    let mut f = Fixture::new();
    f.account(true);
    let output = f.path("delta");
    let result = f.run(&[
        "toolkit",
        "export-delta-native",
        arg(&output),
        "--users",
        USER,
        "--start",
        "100",
        "--run-id",
        "broken",
    ]);
    assert!(!result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["success"], false);
    assert_eq!(report["messages_exported"], 0);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(report["manifest_path"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 0);
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read_dir(output.join("deltas/broken/chats"))
            .unwrap()
            .count(),
        0
    );
    f.unchanged();
}

#[cfg(windows)]
#[test]
fn delta_and_plan_reject_junction_output_ancestors() {
    let mut f = Fixture::new();
    f.account(false);
    let cache = f.cache();
    let outside = f.path("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"KEEP").unwrap();
    let link = f.path("junction");
    let _junction = junction(&link, &outside);
    failed(f.run(&[
        "toolkit",
        "export-delta-native",
        arg(&link.join("new")),
        "--users",
        USER,
        "--start",
        "100",
        "--run-id",
        "safe",
    ]));
    failed(f.run(&[
        "toolkit",
        "chat-plan-native",
        "--decrypted-dir",
        arg(&cache),
        "--user",
        "alpha",
        "--output",
        arg(&link.join("plan.csv")),
    ]));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"KEEP");
    f.unchanged();
}

#[cfg(windows)]
#[test]
fn plan_rejects_junction_ancestors_of_decrypted_source() {
    let mut f = Fixture::new();
    f.cache();
    let link = f.path("junction");
    let _junction = junction(&link, f.root.path());
    let aliased_cache = link.join("cache");
    let output = f.path("plan.csv");
    let result = f.run(&[
        "toolkit",
        "chat-plan-native",
        "--decrypted-dir",
        arg(&aliased_cache),
        "--message-db",
        "messages.db",
        "--user",
        "alpha",
        "--output",
        arg(&output),
    ]);
    f.unchanged();
    // 末级普通目录不能掩盖解密来源祖先上的 junction。
    failed(result);
    assert!(!output.exists());
}

#[cfg(windows)]
struct Junction(PathBuf);

#[cfg(windows)]
impl Drop for Junction {
    fn drop(&mut self) {
        // 仅删除目录项，不递归访问 junction 目标。
        let _ = fs::remove_dir(&self.0);
    }
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) -> Junction {
    println!("COMMAND: cmd.exe /d /c mklink /J {:?} {:?}", link, target);
    let result = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J", arg(link), arg(target)])
        .output()
        .unwrap();
    println!(
        "STDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.status.success());
    Junction(link.to_owned())
}

fn reserved_database(path: &Path) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch("PRAGMA page_size=4096").unwrap();
    let mut reserve: i32 = 80;
    // 与 SQLCipher 页格式一致，只对测试自建库预留加密尾部。
    let status = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            db.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut i32).cast(),
        )
    };
    assert_eq!(status, rusqlite::ffi::SQLITE_OK);
    db
}
fn encrypt(plain: &Path, target: &Path) {
    let plain = fs::read(plain).unwrap();
    assert_eq!(plain[20], 80);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mut mac_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&key, &salt.map(|b| b ^ 0x3a), 2, &mut mac_key);
    let mut encrypted = Vec::new();
    for (index, bytes) in plain.chunks_exact(4096).enumerate() {
        let start = if index == 0 { 16 } else { 0 };
        let mut page = [0u8; 4096];
        if index == 0 {
            page[..16].copy_from_slice(&salt);
        }
        let iv = [0x55u8; 16];
        page[start..4016].copy_from_slice(
            &cbc::Encryptor::<aes::Aes256>::new((&key).into(), (&iv).into())
                .encrypt_padded_vec_mut::<NoPadding>(&bytes[start..4016]),
        );
        page[4016..4032].copy_from_slice(&iv);
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[start..4032]);
        mac.update(&((index + 1) as u32).to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        encrypted.extend_from_slice(&page);
    }
    fs::write(target, encrypted).unwrap();
}
