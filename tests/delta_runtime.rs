//! 真实 wx 进程及 IPC 的 delta 回归；账号、数据库和密钥全部为临时合成数据。
#![cfg(windows)]
#[path = "support/key_store.rs"]
mod key_store_fixture;

use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use sha2::Sha512;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

// 此 fixture 来自旧 export_delta_one 的 AST oracle，不由生产 Rust UID 算法生成。
fn golden() -> Value {
    serde_json::from_str(include_str!("fixtures/delta-query/golden.json")).unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    profile: PathBuf,
}

impl Fixture {
    fn unconfigured() -> Self {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("wxid_self_abcd");
        Self { root, profile }
    }

    fn encrypted_account() -> Self {
        let fixture = Self::unconfigured();
        let account = &fixture.profile;
        let storage = account.join("db_storage");
        for directory in ["contact", "session", "message"] {
            fs::create_dir_all(storage.join(directory)).unwrap();
        }
        let g = golden();
        let mut keys = Map::new();
        let contact_plain = account.join("synthetic-contact.db");
        let contact = sqlite_with_reserve(&contact_plain);
        contact.execute_batch("CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER, description TEXT, local_type INTEGER, extra_buffer BLOB); CREATE TABLE contact_label(label_id_,label_name_,sort_order_);").unwrap();
        for (username, display) in g["names"].as_object().unwrap() {
            let metadata = g["contacts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["username"] == *username);
            let nickname = metadata
                .and_then(|c| c["nick_name"].as_str())
                .unwrap_or(display.as_str().unwrap());
            let description = metadata
                .and_then(|c| c["description"].as_str())
                .unwrap_or("");
            contact
                .execute(
                    "INSERT INTO contact VALUES(?1,?2,?3,0,?4,0,NULL)",
                    rusqlite::params![username, nickname, display.as_str().unwrap(), description],
                )
                .unwrap();
        }
        drop(contact);
        encrypt_fixture(&contact_plain, &storage.join("contact/contact.db"));
        keys.insert("contact/contact.db".into(), json!("11".repeat(32)));

        let session_plain = account.join("synthetic-session.db");
        let session = sqlite_with_reserve(&session_plain);
        session
            .execute_batch(
                "CREATE TABLE SessionTable(username TEXT,type INTEGER,last_timestamp INTEGER)",
            )
            .unwrap();
        for case in g["cases"].as_array().unwrap() {
            session
                .execute(
                    "INSERT INTO SessionTable VALUES(?1,0,200)",
                    [case["username"].as_str().unwrap()],
                )
                .unwrap();
        }
        drop(session);
        encrypt_fixture(&session_plain, &storage.join("session/session.db"));
        keys.insert("session/session.db".into(), json!("11".repeat(32)));

        for (index, shard) in g["shards"].as_array().unwrap().iter().enumerate() {
            let source = shard["source"].as_str().unwrap();
            let plain = account.join(format!("synthetic-message-{index}.db"));
            let conn = sqlite_with_reserve(&plain);
            conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT)")
                .unwrap();
            for (id, username) in shard["ids"].as_object().unwrap() {
                conn.execute(
                    "INSERT INTO Name2Id(rowid,user_name) VALUES(?1,?2)",
                    rusqlite::params![id.parse::<i64>().unwrap(), username.as_str().unwrap()],
                )
                .unwrap();
            }
            for (username, rows) in shard["tables"].as_object().unwrap() {
                let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
                conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER)")).unwrap();
                for row in rows.as_array().unwrap() {
                    let raw = match row["raw"]["kind"].as_str().unwrap() {
                        "null" => rusqlite::types::Value::Null,
                        "text" => rusqlite::types::Value::Text(
                            row["raw"]["value"].as_str().unwrap().into(),
                        ),
                        "bytes" => rusqlite::types::Value::Blob(
                            serde_json::from_value(row["raw"]["value"].clone()).unwrap(),
                        ),
                        kind => panic!("未知合成正文类型: {kind}"),
                    };
                    conn.execute(
                        &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,?4,?5,?6)"),
                        rusqlite::params![
                            row["local_id"].as_i64(),
                            row["local_type"].as_i64(),
                            row["timestamp"].as_i64(),
                            row["sender_id"].as_i64(),
                            raw,
                            row["compression"].as_i64()
                        ],
                    )
                    .unwrap();
                }
            }
            drop(conn);
            encrypt_fixture(&plain, &storage.join(source));
            keys.insert(source.into(), json!("11".repeat(32)));
        }
        fs::write(
            account.join("all_keys.json"),
            serde_json::to_vec(&keys).unwrap(),
        )
        .unwrap();
        fs::write(account.join("config.json"), json!({"db_dir":"db_storage", "keys_file":"all_keys.json", "decrypted_dir":"decrypted"}).to_string()).unwrap();
        crate::key_store_fixture::migrate(
            std::path::Path::new(env!("CARGO_BIN_EXE_wx")),
            &account.join("config.json"),
            &fixture.runtime_root(),
        );
        fixture
    }

    fn runtime_root(&self) -> PathBuf {
        self.root.path().join("isolated-runtime")
    }

    fn output(&self, label: &str) -> PathBuf {
        self.root.path().join(label)
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(args)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_USERS")
            .env("WX_CLI_CONFIG", self.profile.join("config.json"))
            .env("WX_CLI_HOME", self.runtime_root())
            .env(
                "WX_WECHAT_DECRYPT_PYTHON",
                self.root.path().join("not-installed-python.exe"),
            )
            .current_dir(self.root.path());
        eprintln!("执行命令：{command:?}");
        let result = command.output().unwrap();
        eprintln!(
            "退出码：{:?}\n完整标准输出：\n{}\n完整错误输出：\n{}",
            result.status.code(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    }

    fn delta(
        &self,
        output: &Path,
        users: &str,
        start: &str,
        end: Option<&str>,
        run_id: Option<&str>,
    ) -> Output {
        let mut args = vec![
            "toolkit",
            "export-delta-native",
            output.to_str().unwrap(),
            "--users",
            users,
            "--start",
            start,
        ];
        if let Some(end) = end {
            args.extend(["--end", end]);
        }
        if let Some(run_id) = run_id {
            args.extend(["--run-id", run_id]);
        }
        self.run(&args)
    }

    fn account_runtime(&self) -> PathBuf {
        let accounts: Vec<_> = fs::read_dir(self.runtime_root().join("accounts"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(accounts.len(), 1, "测试必须只启动自己的合成账号 daemon");
        accounts[0].clone()
    }

    fn append_delta(&self, output: &Path, run_id: &str) -> Output {
        self.run(&[
            "toolkit",
            "export-delta-native",
            output.to_str().unwrap(),
            "--users",
            "wxid_peer,synthetic@chatroom,wxid_empty",
            "--start",
            "100",
            "--end",
            "200",
            "--run-id",
            run_id,
            "--append-run",
        ])
    }

    fn assert_real_daemon_and_decryption(&self) {
        let runtime = self.account_runtime();
        let record = read_json(&runtime.join("daemon.pid"));
        assert!(record["pid"].as_u64().unwrap() > 0);
        assert!(!record["runtime_id"].as_str().unwrap().is_empty());
        let log = fs::read_to_string(runtime.join("daemon.log")).unwrap();
        assert!(log.contains("[daemon] wx-daemon 启动"));
        let mtimes = read_json(&runtime.join("cache/_mtimes.json"));
        for key in [
            "contact/contact.db",
            "message/message_0.db",
            "message/message_1.db",
            "message/message_2.db",
        ] {
            let path = PathBuf::from(mtimes[key]["path"].as_str().unwrap());
            // Windows 运行目录可能带 \\?\ 前缀，比较已解析的同一文件系统身份。
            assert!(
                fs::canonicalize(&path)
                    .unwrap()
                    .starts_with(fs::canonicalize(runtime.join("cache")).unwrap()),
                "缓存必须位于测试隔离账号下: {path:?}, runtime={runtime:?}"
            );
            let bytes = fs::read(path).unwrap();
            assert_eq!(&bytes[..16], b"SQLite format 3\0");
        }
    }
}

#[path = "support/bootstrap.rs"]
mod bootstrap;

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.profile.join("config.json").exists() && self.runtime_root().exists() {
            let result = self.run(&["daemon", "stop"]);
            if !result.status.success() {
                eprintln!("警告：合成账号 daemon 停止失败，检查本次完整错误输出");
            }
        }
        drop(bootstrap::RuntimeCleanup(self.runtime_root()));
        // TempDir 只清理本测试创建的绝对临时目录，不接触用户账号及安装目录。
    }
}

fn sqlite_with_reserve(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA page_size=4096;").unwrap();
    let mut reserve: i32 = 80;
    // 与 runtime_isolation.rs 相同，使用 SQLite 官方接口预留 SQLCipher 每页尾部。
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut i32).cast(),
        )
    };
    assert_eq!(result, rusqlite::ffi::SQLITE_OK);
    conn
}

// 固定虚构密钥，仅用于测试数据加密；没有任何生产加密写回或真实账号读取。
fn encrypt_fixture(plain: &Path, output: &Path) {
    let plain = fs::read(plain).unwrap();
    assert_eq!(plain[20], 80);
    assert_eq!(plain.len() % 4096, 0);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mac_salt: Vec<u8> = salt.iter().map(|byte| byte ^ 0x3a).collect();
    let mut mac_key = [0; 32];
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

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "命令应成功：\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI 成功响应必须为 JSON")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest_path(output: &Path, run_id: &str) -> PathBuf {
    output.join("deltas").join(run_id).join("manifest.json")
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    tree_snapshot_except(root, &[])
}

fn tree_snapshot_except(root: &Path, excluded: &[PathBuf]) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(
        root: &Path,
        directory: &Path,
        excluded: &[PathBuf],
        result: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if excluded.contains(&path) {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, excluded, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, excluded, &mut result);
    result
}

fn assert_no_partial_success(output: &Path, run_id: &str, expected_user: &str) -> Value {
    let manifest = read_json(&manifest_path(output, run_id));
    assert_eq!(manifest["chats_checked"], 1);
    assert_eq!(manifest["chats_with_messages"], 0);
    assert_eq!(manifest["messages_exported"], 0);
    assert_eq!(manifest["files"], json!([]));
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["errors"][0]["username"], expected_user);
    assert!(!manifest["errors"][0]["reason"].as_str().unwrap().is_empty());
    assert_eq!(
        tree_snapshot(output).len(),
        1,
        "失败会话不得留下半份 delta 或临时文件"
    );
    manifest
}

#[test]
fn encrypted_account_ipc_delta_matches_raw_uid_oracle_and_never_overwrites() {
    let f = Fixture::encrypted_account();
    let g = golden();
    let users = "wxid_peer,synthetic@chatroom,wxid_empty";
    let output = f.output("delta-success");
    let existing_full = f.output("full.json");
    fs::write(
        &existing_full,
        b"synthetic existing full JSON; must not read or overwrite",
    )
    .unwrap();
    let sources_before = tree_snapshot(&f.profile.join("db_storage"));
    let keys_before = fs::read(f.profile.join("all_keys.json")).unwrap();
    let report = success(f.delta(&output, users, "100", Some("200"), Some("delta-runtime")));
    assert_eq!(report["success"], true);
    let expected_manifest = manifest_path(&output, "delta-runtime");
    assert_eq!(
        Path::new(report["manifest_path"].as_str().unwrap()),
        expected_manifest
    );
    let manifest = read_json(&expected_manifest);
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["export_kind"], "wechat_delta_run");
    assert_eq!(manifest["run_id"], "delta-runtime");
    assert_eq!(manifest["chats_checked"], 3);
    assert_eq!(manifest["chats_with_messages"], 2);
    assert_eq!(
        manifest["messages_exported"],
        g["manifest"]["messages_exported"]
    );
    assert_eq!(manifest["errors"], json!([]));
    assert_eq!(manifest["files"], g["manifest"]["files"]);

    let run = output.join("deltas/delta-runtime");
    for case in &g["cases"].as_array().unwrap()[..2] {
        let path = run.join(case["result"]["path"].as_str().unwrap());
        let document = read_json(&path);
        let mut messages = document["messages"].clone();
        for message in messages.as_array_mut().unwrap() {
            assert!(message["source"]
                .as_str()
                .unwrap()
                .starts_with("message/message_"));
            assert!(
                message.get("raw_content").is_none(),
                "原始字节只用于 UID，不泄露为额外导出正文"
            );
            message.as_object_mut().unwrap().remove("source");
        }
        // 精确比对所有旧消息字段和 SHA256，压缩字节及转账有效类型均来自独立 Python oracle。
        assert_eq!(messages, case["document"]["messages"]);
        for key in [
            "schema_version",
            "export_kind",
            "username",
            "chat",
            "is_group",
            "contact_remark",
            "contact_nick_name",
            "contact_tags",
            "contact_memo",
            "message_count",
        ] {
            assert_eq!(
                document.get(key),
                case["document"].get(key),
                "字段 {key} 不一致"
            );
        }
        assert_eq!(document["range"], manifest["range"]);
        for key in ["exported_at", "date_first_msg", "date_last_msg"] {
            chrono::NaiveDateTime::parse_from_str(
                document[key].as_str().unwrap(),
                "%Y-%m-%d %H:%M:%S",
            )
            .unwrap();
        }
        if case["username"] == "wxid_peer" {
            let same_ids: Vec<_> = document["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["local_id"] == 9)
                .collect();
            assert_eq!(same_ids.len(), 2, "跨分片同号不能误去重");
            assert_ne!(same_ids[0]["source"], same_ids[1]["source"]);
            assert_ne!(same_ids[0]["msg_uid"], same_ids[1]["msg_uid"]);
            assert_eq!(document["messages"][0]["timestamp"], 100);
            assert_eq!(
                document["messages"].as_array().unwrap().last().unwrap()["timestamp"],
                200
            );
        } else {
            assert_eq!(document["messages"][0]["content"], "压缩正文不是摘要");
            assert_eq!(document["messages"][0]["sender"], "合成群成员");
            assert!(document["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["type"] == "transfer"));
        }
    }
    f.assert_real_daemon_and_decryption();
    let original = tree_snapshot(&output);
    assert_eq!(original.len(), 3, "只应有两份非空 delta 和一个 manifest");
    let repeated = f.delta(&output, users, "100", Some("200"), Some("delta-runtime"));
    assert!(!repeated.status.success());
    assert_eq!(tree_snapshot(&output), original);
    let another_run = f.delta(&output, users, "100", Some("200"), Some("another-run"));
    assert!(
        !another_run.status.success(),
        "已有 root 不得通过更换 run-id 绕过独占限制"
    );
    assert_eq!(tree_snapshot(&output), original);
    assert!(!f
        .delta(
            &existing_full,
            "wxid_peer",
            "100",
            Some("200"),
            Some("existing-file")
        )
        .status
        .success());
    assert_eq!(
        fs::read(existing_full).unwrap(),
        b"synthetic existing full JSON; must not read or overwrite"
    );
    assert_eq!(tree_snapshot(&f.profile.join("db_storage")), sources_before);
    assert_eq!(
        fs::read(f.profile.join("all_keys.json")).unwrap(),
        keys_before
    );
}

#[test]
fn empty_window_and_default_run_id_publish_successful_empty_manifests() {
    let f = Fixture::encrypted_account();
    let output = f.output("empty-window");
    let result = success(f.delta(
        &output,
        "wxid_peer,synthetic@chatroom,wxid_empty",
        "1000",
        Some("2000"),
        Some("empty-window"),
    ));
    assert_eq!(result["success"], true);
    let manifest = read_json(&manifest_path(&output, "empty-window"));
    assert_eq!(manifest["chats_checked"], 3);
    assert_eq!(manifest["chats_with_messages"], 0);
    assert_eq!(manifest["messages_exported"], 0);
    assert_eq!(manifest["files"], json!([]));
    assert_eq!(manifest["errors"], json!([]));
    assert_eq!(tree_snapshot(&output).len(), 1);
    let automatic = f.output("default-run-id");
    let result = success(f.delta(&automatic, "wxid_peer", "1000", None, None));
    assert_eq!(result["success"], true);
    let path = Path::new(result["manifest_path"].as_str().unwrap());
    assert!(path.starts_with(automatic.join("deltas")));
    let manifest = read_json(path);
    let run_id = manifest["run_id"].as_str().unwrap();
    assert!(!run_id.is_empty());
    assert_eq!(path, manifest_path(&automatic, run_id));
    assert_eq!(manifest["range"]["end"], "");
    assert_eq!(tree_snapshot(&automatic).len(), 1);
}

#[test]
fn invalid_delta_windows_and_run_ids_fail_before_account_or_daemon_access() {
    let f = Fixture::unconfigured();
    let output = f.output("must-not-exist");
    let reversed = f.delta(&output, "wxid_peer", "200", Some("100"), Some("reversed"));
    assert!(!reversed.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&reversed.stdout),
        String::from_utf8_lossy(&reversed.stderr)
    );
    assert!(
        !text.contains("config.json"),
        "应在读取配置前拒绝反向日期: {text}"
    );
    assert!(
        !text.contains("unrecognized subcommand"),
        "公共 CLI 尚未注册，不能视为参数校验通过"
    );
    assert!(!output.exists());
    assert!(!f.runtime_root().exists());
    assert!(!f.profile.exists());
    let missing = f.run(&[
        "toolkit",
        "export-delta-native",
        output.to_str().unwrap(),
        "--users",
        "wxid_peer",
    ]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--start"));
    assert!(!output.exists());
    assert!(!f.runtime_root().exists());
    let invalid = f.delta(&output, "wxid_peer", "100", Some("200"), Some("../escape"));
    assert!(!invalid.status.success());
    assert!(!output.exists());
    assert!(!f.output("escape").exists());
    assert!(!f.runtime_root().exists());
}

#[test]
fn disappeared_known_shard_fails_without_publishing_a_partial_chat() {
    let f = Fixture::encrypted_account();
    let warmup = f.output("warmup");
    success(f.delta(&warmup, "wxid_peer", "100", Some("200"), Some("warmup")));
    let warmup_before = tree_snapshot(&warmup);
    f.assert_real_daemon_and_decryption();
    let runtime = f.account_runtime();
    let before = fs::read(runtime.join("daemon.pid")).unwrap();
    let path = f.profile.join("db_storage/message/message_1.db");
    fs::remove_file(path).unwrap();
    let output = f.output("missing-shard");
    let result = f.delta(
        &output,
        "wxid_peer",
        "100",
        Some("200"),
        Some("missing-shard"),
    );
    assert!(!result.status.success(), "缺失已知分片必须返回非零退出码");
    let manifest = assert_no_partial_success(&output, "missing-shard", "wxid_peer");
    assert_eq!(
        manifest["errors"][0]["reason"],
        "delta query error: Business operation failed"
    );
    assert_eq!(
        fs::read(runtime.join("daemon.pid")).unwrap(),
        before,
        "应通过同一 daemon 的已知分片快照发现缺失"
    );
    assert_eq!(tree_snapshot(&warmup), warmup_before);
    assert_eq!(warmup_before.len(), 2);
    assert_eq!(
        read_json(&manifest_path(&warmup, "warmup"))["errors"],
        json!([])
    );
}

#[test]
fn encrypted_shard_without_a_key_is_an_error_not_a_successful_subset() {
    let f = Fixture::encrypted_account();
    let keys_path = f.profile.join("all_keys.json");
    let mut keys = read_json(&keys_path);
    keys.as_object_mut().unwrap().remove("message/message_1.db");
    fs::write(&keys_path, serde_json::to_vec(&keys).unwrap()).unwrap();
    fs::remove_file(f.profile.join("keys.dpapi")).unwrap();
    key_store_fixture::migrate(
        Path::new(env!("CARGO_BIN_EXE_wx")),
        &f.profile.join("config.json"),
        &f.runtime_root(),
    );
    let output = f.output("unkeyed-shard");
    let result = f.delta(
        &output,
        "wxid_peer",
        "100",
        Some("200"),
        Some("unkeyed-shard"),
    );
    assert!(!result.status.success());
    let manifest = assert_no_partial_success(&output, "unkeyed-shard", "wxid_peer");
    assert_eq!(
        manifest["errors"][0]["reason"],
        "delta query error: Business operation failed"
    );
    assert_eq!(
        fs::read(&keys_path).unwrap(),
        serde_json::to_vec(&keys).unwrap()
    );
}

#[test]
fn append_run_preserves_full_exports_and_previous_runs_byte_for_byte() {
    let f = Fixture::encrypted_account();
    let output = f.output("existing-export");
    fs::create_dir(&output).unwrap();
    let full = output.join("single_existing_full.json");
    fs::write(
        &full,
        b"synthetic full chat: deliberately not valid JSON\r\n",
    )
    .unwrap();
    let old_full = tree_snapshot(&output);
    let sources = tree_snapshot(&f.profile);
    for run_id in ["append-first", "append-second"] {
        let previous = tree_snapshot(&output);
        let report = success(f.append_delta(&output, run_id));
        assert_eq!(report["success"], true);
        assert_eq!(
            Path::new(report["manifest_path"].as_str().unwrap()),
            manifest_path(&output, run_id)
        );
        let manifest = read_json(&manifest_path(&output, run_id));
        assert_eq!(manifest["run_id"], run_id);
        assert_eq!(manifest["chats_checked"], 3);
        assert_eq!(manifest["chats_with_messages"], 2);
        assert_eq!(manifest["files"], golden()["manifest"]["files"]);
        assert_eq!(
            manifest["messages_exported"],
            golden()["manifest"]["messages_exported"]
        );
        assert_eq!(manifest["errors"], json!([]));
        let after = tree_snapshot(&output);
        assert_eq!(after.len(), previous.len() + 3);
        for (path, bytes) in previous {
            assert_eq!(after.get(&path), Some(&bytes), "old file changed: {path:?}");
        }
    }
    f.assert_real_daemon_and_decryption();
    assert_eq!(
        fs::read(&full).unwrap(),
        old_full[Path::new("single_existing_full.json")]
    );
    let before_rejection = tree_snapshot(&output);
    for run_id in ["append-first", "append-second"] {
        let result = f.append_delta(&output, run_id);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("delta run must be new and exclusive")
        );
        assert_eq!(tree_snapshot(&output), before_rejection);
    }
    assert_eq!(fs::read_dir(output.join("deltas")).unwrap().count(), 2);
    assert_eq!(tree_snapshot(&f.profile), sources);
}

#[test]
fn append_run_requires_an_existing_root_without_loading_account_databases() {
    let f = Fixture::encrypted_account();
    let output = f.output("absent-append-root");
    let sources = tree_snapshot(&f.profile);
    let result = f.append_delta(&output, "missing-root");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
    assert!(!stderr.contains("unrecognized subcommand"), "{stderr}");
    assert!(!stderr.contains("启动当前账号"), "{stderr}");
    assert!(!output.exists());
    assert_eq!(tree_snapshot(&f.profile), sources);
    let runtime = f.account_runtime();
    assert_eq!(fs::read_dir(runtime.join("cache")).unwrap().count(), 0);
    assert!(!f.runtime_root().join("bootstrap").exists());
}

#[test]
fn append_run_rejects_source_decrypted_cache_and_runtime_directories() {
    let f = Fixture::encrypted_account();
    success(f.delta(
        &f.output("warmup"),
        "wxid_peer",
        "100",
        Some("200"),
        Some("warmup"),
    ));
    f.assert_real_daemon_and_decryption();
    let runtime = f.account_runtime();
    assert!(f.run(&["daemon", "stop"]).status.success());
    let lifecycle: Vec<_> = [
        "daemon.pid",
        "daemon.log",
        "daemon.lock",
        "startup.lock",
        "service-token.key",
    ]
    .iter()
    .map(|name| runtime.join(name))
    .collect();
    let protected = [
        f.profile.join("db_storage"),
        f.profile.join("decrypted"),
        runtime.join("cache"),
        runtime,
    ];
    for directory in &protected {
        fs::create_dir_all(directory.join("existing-output")).unwrap();
        fs::write(
            directory.join("existing-output/sentinel.json"),
            b"keep protected bytes",
        )
        .unwrap();
    }
    let before = tree_snapshot_except(f.root.path(), &lifecycle);
    for directory in &protected {
        for output in [directory.clone(), directory.join("existing-output")] {
            let result = f.append_delta(&output, "forbidden-run");
            assert!(
                !result.status.success(),
                "protected output accepted: {output:?}"
            );
            assert!(String::from_utf8_lossy(&result.stderr)
                .contains("Output must be outside the source directory"));
            assert!(!output.join("deltas").exists());
            assert_eq!(tree_snapshot_except(f.root.path(), &lifecycle), before);
        }
        let missing = directory.join("missing-output");
        let result = f.delta(
            &missing,
            "wxid_peer",
            "100",
            Some("200"),
            Some("forbidden-run"),
        );
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("Output must be outside the source directory"));
        assert!(!missing.exists());
        assert_eq!(tree_snapshot_except(f.root.path(), &lifecycle), before);
    }
    for output in [
        f.profile.join("all_keys.json"),
        f.profile.join("keys.dpapi"),
        f.profile.join("config.json"),
    ] {
        assert!(!f.append_delta(&output, "forbidden-run").status.success());
        assert_eq!(tree_snapshot_except(f.root.path(), &lifecycle), before);
    }
}
