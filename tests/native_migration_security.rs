//! 原生迁移安全回归：仅合成 SQLite，禁止依赖真实账号、Python 或 ffmpeg。
use rusqlite::{params, Connection};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        fs::write(
            fixture.path("ambient.json"),
            b"invalid ambient account config",
        )
        .unwrap();
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_wx"))
            .args(args)
            .current_dir(self.0.path())
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_CONTACTS")
            .env_remove("WECHAT_EXPORT_USERS")
            .env("WX_CLI_CONFIG", self.path("ambient.json"))
            .env("WX_CLI_HOME", self.path("runtime"))
            .env("WX_WECHAT_DECRYPT_DIR", self.path("absent-toolkit"))
            .env("WX_WECHAT_DECRYPT_PYTHON", self.path("absent-python.exe"))
            .env("PATH", "")
            .output()
            .unwrap();
        assert_eq!(
            fs::read(self.path("ambient.json")).unwrap(),
            b"invalid ambient account config"
        );
        assert!(!self.path("runtime").exists(), "离线入口不得启动账号运行时");
        output
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn failure(output: Output) -> String {
    assert!(
        !output.status.success(),
        "意外成功: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn enterprise(f: &Fixture) -> PathBuf {
    let root = f.path("snapshot");
    fs::create_dir(&root).unwrap();
    let db = Connection::open(root.join("message.db")).unwrap();
    db.execute_batch("CREATE TABLE message_table(message_id INTEGER,server_id INTEGER,sequence INTEGER,sender_id INTEGER,conversation_id TEXT,content_type INTEGER,send_time INTEGER,flag INTEGER,content TEXT,extra_content TEXT,local_extra_content TEXT);").unwrap();
    for (id, cid, text) in [
        (
            1,
            "../outside' OR 1=1 --",
            "=HYPERLINK(\"https://invalid.example\",\"x\")\r\n<script>&\"'",
        ),
        (2, "other-account", "PRIVATE_OTHER_CONVERSATION"),
    ] {
        db.execute(
            "INSERT INTO message_table VALUES (?1,?1,?1,7,?2,0,100,0,?3,'','')",
            params![id, cid, text],
        )
        .unwrap();
    }
    root
}

#[test]
fn enterprise_injection_is_literal_and_exports_escape_untrusted_cells() {
    let f = Fixture::new();
    let root = enterprise(&f);
    let original = fs::read(root.join("message.db")).unwrap();
    let cid = "../outside' OR 1=1 --";
    let contacts = success(f.run(&["toolkit", "enterprise", arg(&root), "contacts"]));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&contacts).unwrap(),
        serde_json::json!([])
    );
    let conversations = success(f.run(&["toolkit", "enterprise", arg(&root), "conversations"]));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&conversations)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let selected = success(f.run(&[
        "toolkit",
        "enterprise",
        arg(&root),
        "messages",
        "--conversations",
        cid,
    ]));
    let selected: serde_json::Value = serde_json::from_str(&selected).unwrap();
    assert_eq!(selected.as_array().unwrap().len(), 1);
    assert_eq!(selected[0]["conversation_id"], cid);
    let text = selected[0]["content"].as_str().unwrap();
    let missing = success(f.run(&[
        "toolkit",
        "enterprise",
        arg(&root),
        "messages",
        "--conversations",
        "' OR 1=1 --",
    ]));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&missing).unwrap(),
        serde_json::json!([])
    );
    for format in ["csv", "html"] {
        let output = f.path(&format!("export.{format}"));
        success(f.run(&[
            "toolkit",
            "enterprise",
            arg(&root),
            "export",
            cid,
            arg(&output),
            "--format",
            format,
        ]));
        let rendered = fs::read_to_string(&output).unwrap();
        assert!(!rendered.contains("PRIVATE_OTHER_CONVERSATION"));
        if format == "csv" {
            // 用 CSV 解析器验证逗号、引号和换行仍属于同一字段。
            let mut reader =
                csv::Reader::from_reader(rendered.trim_start_matches('\u{feff}').as_bytes());
            let rows = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(&rows[0][6], format!("'{text}"));
        } else {
            assert!(!rendered.contains("<script>"));
            assert!(rendered.contains("&lt;script&gt;&amp;&quot;&#x27;"));
            assert!(rendered.contains("Content-Security-Policy"));
        }
    }
    assert_eq!(fs::read(root.join("message.db")).unwrap(), original);
    assert!(!root.join("user.db").exists());
    assert!(!root.join("session.db").exists());
    assert!(!f.path("outside").exists());
}

#[test]
fn enterprise_rejects_source_aliases_existing_targets_and_missing_databases() {
    let f = Fixture::new();
    let root = enterprise(&f);
    let db = root.join("message.db");
    let before = fs::read(&db).unwrap();
    let occupied = f.path("occupied.json");
    fs::write(&occupied, b"KEEP").unwrap();
    for target in [
        db.clone(),
        root.join("new.json"),
        root.join("../snapshot/alias.json"),
        occupied.clone(),
    ] {
        failure(f.run(&[
            "toolkit",
            "enterprise",
            arg(&root),
            "export",
            "other-account",
            arg(&target),
        ]));
    }
    let unknown = f.path("unknown.json");
    failure(f.run(&[
        "toolkit",
        "enterprise",
        arg(&root),
        "export",
        "unknown",
        arg(&unknown),
    ]));
    assert!(!unknown.exists());
    assert_eq!(fs::read(&occupied).unwrap(), b"KEEP");
    assert_eq!(fs::read(&db).unwrap(), before);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    let empty = f.path("empty-snapshot");
    fs::create_dir(&empty).unwrap();
    for operation in ["contacts", "conversations", "messages"] {
        failure(f.run(&["toolkit", "enterprise", arg(&empty), operation]));
    }
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
}

#[test]
fn voice_explicit_config_preserves_foreign_owner_and_existing_mp3_without_tools() {
    use chrono::TimeZone;
    let f = Fixture::new();
    let config_dir = f.path("account");
    fs::create_dir_all(config_dir.join("decrypted/message")).unwrap();
    let media = config_dir.join("decrypted/message/media_0.db");
    let db = Connection::open(&media).unwrap();
    db.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('alice'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,local_id INTEGER,voice_data BLOB); INSERT INTO VoiceInfo VALUES (1,100,1,NULL);").unwrap();
    drop(db);
    let original = fs::read(&media).unwrap();
    let config = config_dir.join("voice.json");
    let config_bytes = br#"{"decrypted_dir":"decrypted","output_base_dir":"output"}"#;
    fs::write(&config, config_bytes).unwrap();
    let foreign = config_dir.join("output/alice");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join(".info"), b"username:  somebody-else\n").unwrap();
    fs::write(foreign.join("keep.mp3"), b"FOREIGN").unwrap();
    let own = config_dir.join(format!(
        "output/alice~{}",
        &format!("{:x}", md5::compute("alice"))[..12]
    ));
    fs::create_dir_all(own.join("voice")).unwrap();
    fs::write(own.join(".info"), b"username:  alice\n").unwrap();
    let stamp = chrono::Local
        .timestamp_opt(100, 0)
        .single()
        .unwrap()
        .format("%Y%m%d_%H%M%S");
    let mp3 = own.join(format!("voice/{stamp}_1.mp3"));
    fs::write(&mp3, b"EXISTING").unwrap();
    let report = success(f.run(&[
        "toolkit",
        "voice-batch",
        "--config",
        arg(&config),
        "--contacts",
        "alice",
    ]));
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["skipped_existing"], 1);
    assert_eq!(report["converted"], 0);
    assert_eq!(report["failed"], 0);
    for target in [config_dir.join("decrypted/new")] {
        failure(f.run(&[
            "toolkit",
            "voice-batch",
            "--config",
            arg(&config),
            "--output-dir",
            arg(&target),
        ]));
        assert!(!target.exists());
    }
    assert_eq!(fs::read(&mp3).unwrap(), b"EXISTING");
    assert_eq!(
        fs::read(foreign.join(".info")).unwrap(),
        b"username:  somebody-else\n"
    );
    assert_eq!(fs::read(foreign.join("keep.mp3")).unwrap(), b"FOREIGN");
    assert_eq!(fs::read(&media).unwrap(), original);
    assert_eq!(fs::read(&config).unwrap(), config_bytes);
    assert!(!f.path("output").exists());
    assert!(!config_dir.join("decrypted/contact").exists());
}

#[test]
fn sns_rejects_secret_and_cache_boundaries_before_creating_output() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    let sns = f.path("source/sns.db");
    let db = Connection::open(&sns).unwrap();
    db.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT);")
        .unwrap();
    db.execute("INSERT INTO SnsTimeLine VALUES (1,'synthetic',?1)", ["<x><TimelineObject><id>1</id><username>synthetic</username><createTime>1710000000</createTime><contentDesc>boundary</contentDesc><ContentObject><contentStyle>1</contentStyle></ContentObject></TimelineObject></x>"]).unwrap();
    drop(db);
    let original = fs::read(&sns).unwrap();
    let cache = f.path("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("sentinel"), b"CACHE").unwrap();
    let key = f.path("key.txt");
    let secret = "SYNTHETIC_SECRET_DO_NOT_ECHO_012345";
    fs::write(&key, secret).unwrap();
    let output = f.path("output");
    for options in [
        vec!["--image-key-file", arg(&key)],
        vec![
            "--xwechat-cache",
            arg(&cache),
            "--image-key-file",
            arg(&key),
        ],
        vec!["--sns-cache", arg(&cache), "--image-xor-key", "256"],
        vec!["--sns-cache", "missing-cache"],
    ] {
        let mut args = vec!["toolkit", "export-sns-native", arg(&sns), arg(&output)];
        args.extend(options);
        let error = failure(f.run(&args));
        assert!(!error.contains(secret));
        assert!(!output.exists());
    }
    for target in [
        f.path("source/export"),
        cache.join("export"),
        f.path("source/../source/escape"),
    ] {
        failure(f.run(&[
            "toolkit",
            "export-sns-native",
            arg(&sns),
            arg(&target),
            "--xwechat-cache",
            arg(&cache),
        ]));
        assert!(!target.exists());
    }
    let occupied = output.join("synthetic/SNS");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("timeline.json"), b"KEEP").unwrap();
    failure(f.run(&[
        "toolkit",
        "export-sns-native",
        arg(&sns),
        arg(&output),
        "--sns-cache",
        arg(&cache),
    ]));
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&occupied).unwrap().count(), 1);
    assert_eq!(fs::read(occupied.join("timeline.json")).unwrap(), b"KEEP");
    assert_eq!(fs::read(&sns).unwrap(), original);
    assert_eq!(fs::read(&key).unwrap(), secret.as_bytes());
    assert_eq!(fs::read(cache.join("sentinel")).unwrap(), b"CACHE");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
}

#[test]
fn voice_rejects_parent_traversal_before_any_directory_creation() {
    let f = Fixture::new();
    fs::create_dir_all(f.path("decrypted/message")).unwrap();
    fs::create_dir(f.path("allowed")).unwrap();
    let media = f.path("decrypted/message/media_0.db");
    let db = Connection::open(&media).unwrap();
    db.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('alice'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,local_id INTEGER,voice_data BLOB); INSERT INTO VoiceInfo VALUES (1,100,1,NULL);").unwrap();
    drop(db);
    let original = fs::read(&media).unwrap();
    let config = f.path("voice.json");
    fs::write(
        &config,
        br#"{"decrypted_dir":"decrypted","output_base_dir":"allowed"}"#,
    )
    .unwrap();
    let traversal = f.path("allowed/../escaped");
    let error = failure(f.run(&[
        "toolkit",
        "voice-batch",
        "--config",
        arg(&config),
        "--output-dir",
        arg(&traversal),
    ]));
    assert_eq!(fs::read(&media).unwrap(), original);
    // 不能先归一化掉父目录分量，再执行路径安全校验。
    assert!(
        !f.path("escaped").exists(),
        "父目录穿越已经产生写入，stderr/stdout: {error}"
    );
    assert!(
        error.contains("parent"),
        "应在读取语音数据前拒绝路径: {error}"
    );
}

#[test]
fn invalid_chat_dates_fail_before_account_loading_or_output_changes() {
    let f = Fixture::new();
    let output = f.path("export");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("_export_index.json"), b"KEEP INDEX").unwrap();
    for (start, end) in [
        ("2026-02-30", "3"),
        ("9223372036854775807", "3"),
        ("3", "2"),
        ("1", "1; echo injected"),
    ] {
        let error = failure(f.run(&[
            "toolkit",
            "export-chats-native",
            arg(&output),
            "--start",
            start,
            "--end",
            end,
        ]));
        assert!(error.contains("时间"), "{error}");
        assert_eq!(
            fs::read(output.join("_export_index.json")).unwrap(),
            b"KEEP INDEX"
        );
        assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    }
}
