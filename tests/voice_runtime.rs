//! 合成加密账号的真实 daemon / named pipe 回归，不读取真实微信数据。
#![cfg(windows)]
#[path = "support/key_store.rs"]
mod key_store_fixture;
#[path = "fixtures/query_v3.rs"]
mod query_v3;
use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256, Sha512};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Account {
    root: tempfile::TempDir,
    profile: PathBuf,
    home: PathBuf,
    pipe: String,
    child: Option<Child>,
}

impl Account {
    fn new(home: &Path, extra_bytes: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("synthetic-account");
        fs::create_dir_all(profile.join("db_storage/contact")).unwrap();
        fs::create_dir_all(profile.join("db_storage/message")).unwrap();
        fs::create_dir_all(home).unwrap();
        let plain = profile.join("contact-plain.db");
        let conn = sqlite(&plain);
        conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER); INSERT INTO contact VALUES('peer','Synthetic Peer','',0),('other','Other','',0);").unwrap();
        drop(conn);
        encrypt(&plain, &profile.join("db_storage/contact/contact.db"));
        let mut keys = Map::new();
        keys.insert("contact/contact.db".into(), json!("11".repeat(32)));
        // 同时覆盖原始反斜杠和大小写键，磁盘文件仍为标准拼写。
        for (shard, key) in [(0, "message\\media_0.db"), (1, "MeSsAgE/MeDiA_1.Db")] {
            let plain = profile.join(format!("media-{shard}-plain.db"));
            let conn = sqlite(&plain);
            conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,voice_data BLOB);").unwrap();
            for i in 0..15i64 {
                let size = (i as usize + extra_bytes) % 19;
                let data = if i == 0 && shard == 0 {
                    None
                } else {
                    Some(vec![0x41u8; size])
                };
                conn.execute("INSERT INTO VoiceInfo(rowid,chat_name_id,local_id,create_time,voice_data) VALUES(?1,7,?2,?3,?4)",rusqlite::params![i+1,i+1,100+i*2+shard,data]).unwrap();
            }
            conn.execute_batch("INSERT INTO VoiceInfo VALUES(8,999,999,x'ffff');")
                .unwrap();
            drop(conn);
            encrypt(
                &plain,
                &profile.join(format!("db_storage/message/media_{shard}.db")),
            );
            keys.insert(key.into(), json!("11".repeat(32)));
        }
        fs::write(
            profile.join("all_keys.json"),
            serde_json::to_vec(&keys).unwrap(),
        )
        .unwrap();
        fs::write(
            profile.join("config.json"),
            json!({"db_dir":"db_storage","keys_file":"all_keys.json","decrypted_dir":"decrypted"})
                .to_string(),
        )
        .unwrap();
        key_store_fixture::migrate(
            Path::new(env!("CARGO_BIN_EXE_wx")),
            &profile.join("config.json"),
            home,
        );
        let mut digest = Sha256::new();
        digest.update(b"wx-cli-runtime-v2\0");
        for path in [
            profile.join("config.json"),
            profile.join("db_storage"),
            profile.join("all_keys.json"),
            home.to_owned(),
        ] {
            digest.update(
                path.canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .to_lowercase()
                    .as_bytes(),
            );
            digest.update([0]);
        }
        Self {
            root,
            profile,
            home: home.to_owned(),
            pipe: format!("wx-cli-v2-{:x}", digest.finalize()),
            child: None,
        }
    }

    fn start(&mut self) {
        use std::os::windows::process::CommandExt;
        let log = fs::File::create(self.root.path().join("daemon-output.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .env_clear()
            .env("WX_DAEMON_MODE", "1")
            .env("WX_CLI_CONFIG", self.profile.join("config.json"))
            .env("WX_CLI_HOME", &self.home)
            .env("PATH", "")
            .current_dir(self.root.path())
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .creation_flags(0x08000000);
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            command.env(name, &self.home);
        }
        println!(
            "COMMAND: {command:?}; synthetic config={:?}; home={:?}",
            self.profile.join("config.json"),
            self.home
        );
        self.child = Some(command.spawn().unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(value) = self.try_request(json!({"cmd":"ping"})) {
                assert_eq!(value["pong"], true);
                break;
            }
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon 提前退出"
            );
            assert!(std::time::Instant::now() < deadline, "daemon 未就绪");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn try_request(&self, request: Value) -> Result<Value, String> {
        use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced};
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(8), async {
                    let name = self
                        .pipe
                        .as_str()
                        .to_ns_name::<GenericNamespaced>()
                        .map_err(|e| e.to_string())?;
                    let stream = interprocess::local_socket::tokio::Stream::connect(name)
                        .await
                        .map_err(|e| e.to_string())?;
                    query_v3::exchange(stream, &self.pipe, request).await
                })
                .await
                .map_err(|e| e.to_string())?
            })
    }

    fn request(&self, fields: Value) -> Value {
        let mut request = json!({"cmd":"voice_messages","chat":"peer"});
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        self.try_request(request).unwrap()
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, result);
                } else {
                    result.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        let mut result = BTreeMap::new();
        visit(&self.profile.join("db_storage"), &mut result);
        for name in ["all_keys.json", "config.json"] {
            let p = self.profile.join(name);
            result.insert(p.clone(), fs::read(p).unwrap());
        }
        result
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // 仅回收本 fixture 持有句柄的子进程，失败路径同样执行。
            let _ = child.kill();
            let status = child.wait();
            println!("DAEMON EXIT: {status:?}");
            println!(
                "DAEMON STDOUT/STDERR:\n{}",
                fs::read_to_string(self.root.path().join("daemon-output.log")).unwrap_or_default()
            );
        }
    }
}

fn sqlite(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA page_size=4096;").unwrap();
    let mut reserve = 80i32;
    // SQLite 官方接口预留 SQLCipher 的 IV 与 HMAC 页尾。
    let rc = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut i32).cast(),
        )
    };
    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
    conn
}

fn encrypt(plain: &Path, output: &Path) {
    let plain = fs::read(plain).unwrap();
    assert_eq!(plain[20], 80);
    assert_eq!(plain.len() % 4096, 0);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mac_salt: Vec<_> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut mac_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&key, &mac_salt, 2, &mut mac_key);
    let mut encrypted = Vec::new();
    for (index, plain) in plain.chunks_exact(4096).enumerate() {
        let start = if index == 0 { 16 } else { 0 };
        let mut page = [0u8; 4096];
        if index == 0 {
            page[..16].copy_from_slice(&salt);
        }
        let iv = [0x55u8; 16];
        let ciphertext = cbc::Encryptor::<aes::Aes256>::new((&key).into(), (&iv).into())
            .encrypt_padded_vec_mut::<NoPadding>(&plain[start..4016]);
        page[start..4016].copy_from_slice(&ciphertext);
        page[4016..4032].copy_from_slice(&iv);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[start..4032]);
        mac.update(&((index + 1) as u32).to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        encrypted.extend_from_slice(&page);
    }
    fs::write(output, encrypted).unwrap();
}

fn expected(extra_bytes: usize) -> Vec<Value> {
    // 独立的固定交错顺序，不调用生产查询/排序/解密代码。
    (0..30i64).rev().map(|n| {
        let shard=n%2;let i=n/2;
        json!({"username":"peer","source":format!("message/media_{shard}.db"),"chat_name_id":7,"media_rowid":i+1,"local_id":i+1,"create_time":100+n,"voice_data_bytes":if n==0 {None} else {Some((i as usize+extra_bytes)%19)}})
    }).collect()
}
fn success(reply: Value, rows: &[Value]) {
    assert_eq!(reply, json!({"ok":true,"voices":rows,"count":rows.len()}));
}
fn failure(reply: Value, text: &str) {
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(reply["error"].as_str().unwrap().contains(text), "{reply}");
    assert!(
        reply.get("voices").is_none() && reply.get("count").is_none(),
        "不得返回部分成功: {reply}"
    );
}

#[test]
fn encrypted_keys_global_pagination_defaults_filters_and_null_size() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), 0);
    let before = a.snapshot();
    a.start();
    let rows = expected(0);
    success(a.request(json!({})), &rows[..20]);
    success(a.request(json!({"limit":30})), &rows);
    success(a.request(json!({"limit":7,"offset":9})), &rows[9..16]);
    success(a.request(json!({"limit":500,"offset":30})), &[]);
    success(a.request(json!({"since":110,"until":113})), &rows[16..20]);
    success(a.request(json!({"since":100,"until":100})), &rows[29..]);
    success(
        a.request(json!({"chat":"Synthetic Peer","limit":30})),
        &rows,
    );
    assert_eq!(a.snapshot(), before);
}

#[test]
fn two_accounts_same_ids_remain_isolated() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), 0);
    let mut b = Account::new(home.path(), 7);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    assert_ne!(a.pipe, b.pipe);
    a.start();
    b.start();
    for _ in 0..2 {
        success(a.request(json!({"limit":30})), &expected(0));
        success(b.request(json!({"limit":30})), &expected(7));
    }
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[test]
fn missing_or_unknown_media_shard_never_returns_partial_results() {
    for missing in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let mut a = Account::new(home.path(), 0);
        if missing {
            fs::remove_file(a.profile.join("db_storage/message/media_1.db")).unwrap();
        } else {
            fs::copy(
                a.profile.join("db_storage/message/media_1.db"),
                a.profile.join("db_storage/message/media_9.db"),
            )
            .unwrap();
        }
        let before = a.snapshot();
        a.start();
        failure(a.request(json!({})), "unknown or missing media shard");
        assert_eq!(a.snapshot(), before);
    }
}

#[test]
fn invalid_pagination_and_time_range_are_rejected_without_source_changes() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), 0);
    let before = a.snapshot();
    a.start();
    for fields in [
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"offset":1000001}),
    ] {
        failure(a.request(fields), "分页超出范围");
    }
    failure(
        a.request(json!({"since":200,"until":100})),
        "since exceeds until",
    );
    success(a.request(json!({"limit":1})), &expected(0)[..1]);
    assert_eq!(a.snapshot(), before);
}
