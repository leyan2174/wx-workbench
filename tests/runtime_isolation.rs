//! 使用合成 SQLCipher 数据验证完整进程链路，不读取本机微信资料。

#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "fixtures/daemon-tasks/runtime.rs"]
mod daemon_tasks;
#[path = "support/key_store.rs"]
mod key_store_fixture;

use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    root: PathBuf,
    profiles: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "wx-process-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self {
            root,
            profiles: Vec::new(),
        }
    }

    fn account(&mut self, name: &str, valid: bool) -> PathBuf {
        let profile = self.root.join(name);
        fs::create_dir_all(profile.join("db_storage/contact")).unwrap();
        let plain = profile.join("fixture.db");
        let conn = rusqlite::Connection::open(&plain).unwrap();
        conn.execute_batch("PRAGMA page_size=4096;").unwrap();
        let mut reserve: i32 = 80;
        // SQLite 官方文件控制接口为每页预留 IV 与 HMAC 空间。
        let result = unsafe {
            rusqlite::ffi::sqlite3_file_control(
                conn.handle(),
                c"main".as_ptr(),
                rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
                (&mut reserve as *mut i32).cast(),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        conn.execute_batch("CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER);").unwrap();
        conn.execute("INSERT INTO contact VALUES (?1, ?1, '', 0)", [name])
            .unwrap();
        drop(conn);
        encrypt_fixture(&plain, &profile.join("db_storage/contact/contact.db"));
        let keys = if valid {
            serde_json::json!({"contact/contact.db": "11".repeat(32)})
        } else {
            serde_json::json!({})
        };
        fs::write(profile.join("all_keys.json"), keys.to_string()).unwrap();
        let mut config = serde_json::json!({"db_dir":"db_storage", "keys_file":"all_keys.json", "decrypted_dir":"decrypted"});
        if !valid {
            config["key_store"] = serde_json::json!("keys.dpapi");
        }
        fs::write(profile.join("config.json"), config.to_string()).unwrap();
        self.profiles.push(profile.clone());
        if valid {
            self.seed_keys(&profile, &keys);
        }
        profile
    }

    fn seed_keys(&self, profile: &Path, keys: &serde_json::Value) {
        key_store_fixture::seed(&profile.join("config.json"), keys);
    }

    fn run(&self, profile: &Path, args: &[&str]) -> Output {
        run(&self.root, profile, args)
    }
}

fn run(root: &Path, profile: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wx"))
        .args(args)
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_CONFIG", profile.join("config.json"))
        .env("WX_CLI_HOME", root.join("shared-runtime"))
        .current_dir(root)
        .output()
        .unwrap()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for profile in &self.profiles {
            let _ = self.run(profile, &["daemon", "stop"]);
        }
        drop(bootstrap::BootstrapCleanup(
            self.root.join("shared-runtime"),
        ));
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[track_caller]
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn media_image_decode_cache_is_native_and_validates_arguments_without_python() {
    let fixture = Fixture::new();
    let profile = fixture.root.join("unconfigured");
    let input = fixture.root.join("images");
    let output = fixture.root.join("decoded");
    fs::create_dir_all(input.join("chat/2026-09/Img")).unwrap();
    let plain = b"\x89PNG\r\n\x1a\nsynthetic image";
    fs::write(
        input.join("chat/2026-09/Img/image_t.dat"),
        plain.iter().map(|b| b ^ 0x37).collect::<Vec<_>>(),
    )
    .unwrap();
    success(fixture.run(
        &profile,
        &[
            "media",
            "image",
            "decode-cache",
            "--attach-dir",
            input.to_str().unwrap(),
            "--decoded-dir",
            output.to_str().unwrap(),
        ],
    ));
    assert_eq!(
        fs::read(output.join("chat/2026-09/image.png")).unwrap(),
        plain
    );
    let help = success(fixture.run(&profile, &["media", "image", "decode-cache", "--help"]));
    assert!(help.contains("--attach-dir"));
    let invalid = fixture.run(
        &profile,
        &["media", "image", "decode-cache", "--unsupported-option"],
    );
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--unsupported-option"));
    assert_eq!(
        fs::read(output.join("chat/2026-09/image.png")).unwrap(),
        plain
    );
}

#[test]
fn native_batch_export_keeps_exact_identities_and_legacy_content_omissions() {
    let mut fixture = Fixture::new();
    let profile = fixture.account("batch-owner", true);
    let users = ["alpha-batch", "beta-batch", "not-in-contacts"];
    let conn = rusqlite::Connection::open(profile.join("fixture.db")).unwrap();
    for username in &users[..2] {
        conn.execute(
            "INSERT INTO contact VALUES (?1,'same display','',0)",
            [username],
        )
        .unwrap();
    }
    drop(conn);
    encrypt_fixture(
        &profile.join("fixture.db"),
        &profile.join("db_storage/contact/contact.db"),
    );
    let session_plain = profile.join("sessions-plain.db");
    fs::copy(profile.join("fixture.db"), &session_plain).unwrap();
    let conn = rusqlite::Connection::open(&session_plain).unwrap();
    conn.execute_batch(
        "CREATE TABLE SessionTable(username TEXT,type INTEGER,last_timestamp INTEGER)",
    )
    .unwrap();
    for username in users {
        conn.execute("INSERT INTO SessionTable VALUES (?1,0,NULL)", [username])
            .unwrap();
    }
    drop(conn);
    fs::create_dir_all(profile.join("db_storage/session")).unwrap();
    encrypt_fixture(
        &session_plain,
        &profile.join("db_storage/session/session.db"),
    );
    let message_plain = profile.join("messages-plain.db");
    fs::copy(profile.join("fixture.db"), &message_plain).unwrap();
    let conn = rusqlite::Connection::open(&message_plain).unwrap();
    for username in users {
        let table = format!("Msg_{:x}", md5::compute(username));
        conn.execute_batch(&format!("CREATE TABLE [{table}] (local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}] VALUES (1,1,1,NULL,NULL,0),(2,1,2,NULL,'',0),(3,3,3,NULL,'image XML',0),(4,1,4,NULL,'synthetic message',0);")).unwrap();
    }
    drop(conn);
    fs::create_dir_all(profile.join("db_storage/message")).unwrap();
    encrypt_fixture(
        &message_plain,
        &profile.join("db_storage/message/message_0.db"),
    );
    let keys = serde_json::json!({
        "contact/contact.db":"11".repeat(32),
        "session/session.db":"11".repeat(32),
        "message/message_0.db":"11".repeat(32),
    });
    fs::write(profile.join("all_keys.json"), keys.to_string()).unwrap();
    fixture.seed_keys(&profile, &keys);
    let output = fixture.root.join("batch-output");
    daemon_tasks::assert_personal_tasks(&fixture, &profile, users[0]);
    let output_arg = output.to_str().unwrap();
    let preview = success(fixture.run(&profile, &["chats", "export", output_arg, "--dry-run"]));
    let preview: serde_json::Value = serde_json::from_str(&preview).unwrap();
    assert_eq!(preview["planned"], 3);
    assert!(!output.exists());
    let filtered = success(fixture.run(
        &profile,
        &[
            "chats",
            "export",
            output_arg,
            "--dry-run",
            "--users",
            "not-in-contacts",
        ],
    ));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&filtered).unwrap()["planned"],
        1
    );
    assert!(!fixture
        .run(
            &profile,
            &["chats", "export", output_arg, "--users", "missing-user"]
        )
        .status
        .success());
    assert!(!output.exists());
    let result = success(fixture.run(&profile, &["chats", "export", output_arg]));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["written"], 3);
    assert_eq!(report["messages"], 12);
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("_export_index.json")).unwrap()).unwrap();
    let first = index["chats"][users[0]]["current_file"].as_str().unwrap();
    let second = index["chats"][users[1]]["current_file"].as_str().unwrap();
    assert_ne!(first.to_lowercase(), second.to_lowercase());
    for username in users {
        let filename = index["chats"][username]["current_file"].as_str().unwrap();
        let chat: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join(filename)).unwrap()).unwrap();
        assert_eq!(chat["username"], username);
        assert_eq!(chat["contact_tags"], serde_json::json!([]));
        assert_eq!(chat["contact_memo"], "");
        assert_eq!(
            chat["contact_nick_name"],
            if username == "not-in-contacts" {
                ""
            } else {
                "same display"
            }
        );
        assert!(chat["messages"][0].get("content").is_none());
        assert_eq!(chat["messages"][1]["content"], "");
        assert_eq!(chat["messages"][2]["type"], "image");
        assert!(chat["messages"][2].get("content").is_none());
        assert_eq!(chat["messages"][3]["content"], "synthetic message");
    }
    let first_path = output.join(first);
    let mut previous: serde_json::Value =
        serde_json::from_slice(&fs::read(&first_path).unwrap()).unwrap();
    previous["custom_field"] = "keep me".into();
    previous["messages"][0]["transcription"] = "existing annotation".into();
    fs::write(&first_path, serde_json::to_vec(&previous).unwrap()).unwrap();
    let repeated = success(fixture.run(
        &profile,
        &[
            "chats",
            "export",
            output_arg,
            "--incremental",
            "--start",
            "2",
            "--end",
            "3",
        ],
    ));
    let repeated: serde_json::Value = serde_json::from_str(&repeated).unwrap();
    assert_eq!(repeated["messages"], 12);
    assert_eq!(repeated["added_messages"], 0);
    let retained: serde_json::Value =
        serde_json::from_slice(&fs::read(&first_path).unwrap()).unwrap();
    assert_eq!(retained["custom_field"], "keep me");
    assert_eq!(
        retained["messages"][0]["transcription"],
        "existing annotation"
    );
    let mut ambiguous = retained;
    ambiguous["messages"][0]
        .as_object_mut()
        .unwrap()
        .remove("source");
    let before = serde_json::to_vec(&ambiguous).unwrap();
    fs::write(&first_path, &before).unwrap();
    assert!(!fixture
        .run(&profile, &["chats", "export", output_arg, "--incremental"])
        .status
        .success());
    assert_eq!(fs::read(&first_path).unwrap(), before);
    let dated = fixture.root.join("batch-dated");
    let dated_arg = dated.to_str().unwrap();
    assert!(!fixture
        .run(
            &profile,
            &["chats", "export", dated_arg, "--start", "3", "--end", "2"]
        )
        .status
        .success());
    assert!(!dated.exists());
    let result = success(fixture.run(
        &profile,
        &["chats", "export", dated_arg, "--start", "2", "--end", "3"],
    ));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["messages"], 6);
    for username in users {
        let filename = index["chats"][username]["current_file"].as_str().unwrap();
        let chat: serde_json::Value =
            serde_json::from_slice(&fs::read(dated.join(filename)).unwrap()).unwrap();
        assert_eq!(chat["messages"][0]["timestamp"], 2);
        assert_eq!(chat["messages"][1]["timestamp"], 3);
        assert!(!chat["date_first_msg"].as_str().unwrap().is_empty());
        assert!(!chat["date_last_msg"].as_str().unwrap().is_empty());
    }
}

#[test]
#[ignore = "requires native ffmpeg in PATH; explicitly run with audio integration"]
fn native_voice_batch_cli_uses_explicit_config_and_skips_existing() {
    let fixture = Fixture::new();
    let profile = fixture.root.join("unconfigured");
    let decrypted = fixture.root.join("voice-snapshot");
    fs::create_dir_all(decrypted.join("message")).unwrap();
    fs::create_dir_all(decrypted.join("contact")).unwrap();
    let media = decrypted.join("message/media_0.db");
    let conn = rusqlite::Connection::open(&media).unwrap();
    conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('synthetic-alice'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,local_id INTEGER,voice_data BLOB);").unwrap();
    let silk =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/tone.silk"))
            .unwrap();
    conn.execute("INSERT INTO VoiceInfo VALUES (1,1,1,?1)", [silk])
        .unwrap();
    drop(conn);
    let contact = rusqlite::Connection::open(decrypted.join("contact/contact.db")).unwrap();
    contact.execute_batch("CREATE TABLE contact(username TEXT,alias TEXT,remark TEXT,nick_name TEXT); INSERT INTO contact VALUES ('synthetic-alice','','','Alice');").unwrap();
    drop(contact);
    let config = fixture.root.join("voice-config.json");
    fs::write(
        &config,
        r#"{"decrypted_dir":"voice-snapshot","output_base_dir":"voice-output"}"#,
    )
    .unwrap();
    let original = fs::read(&media).unwrap();
    let args = [
        "audio",
        "export",
        "--config",
        config.to_str().unwrap(),
        "--contacts",
        "synthetic-alice",
    ];
    let result = success(fixture.run(&profile, &args));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["converted"], 1);
    assert_eq!(report["failed"], 0);
    let voice = fixture.root.join("voice-output/Alice/voice");
    let paths: Vec<_> = fs::read_dir(&voice)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].extension().unwrap(), "mp3");
    let mp3 = fs::read(&paths[0]).unwrap();
    assert!(mp3.len() > 100);
    let result = success(fixture.run(&profile, &args));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["skipped_existing"], 1);
    assert_eq!(report["converted"], 0);
    assert_eq!(fs::read(&paths[0]).unwrap(), mp3);
    assert_eq!(fs::read(&media).unwrap(), original);
    bootstrap::assert_only_bootstrap(&fixture.root.join("shared-runtime"));
    assert!(!profile.join("config.json").exists());
}

#[test]
fn sns_native_cli_exports_offline_and_preserves_existing_results() {
    let fixture = Fixture::new();
    let profile = fixture.root.join("unconfigured");
    let source = fixture.root.join("source");
    fs::create_dir(&source).unwrap();
    let database = source.join("sns.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); CREATE TABLE SnsMessage_tmp3(feed_id INTEGER, create_time INTEGER,type INTEGER,from_username TEXT,from_nickname TEXT,to_username TEXT,to_nickname TEXT,content TEXT,del_status INTEGER);").unwrap();
    conn.execute("INSERT INTO SnsTimeLine VALUES (1,'synthetic',?1)", ["<x><TimelineObject><id>1</id><username>synthetic</username><createTime>1710000000</createTime><contentDesc>synthetic offline post</contentDesc><ContentObject><contentStyle>1</contentStyle></ContentObject></TimelineObject></x>"]).unwrap();
    drop(conn);
    let before = fs::read(&database).unwrap();
    let output = fixture.root.join("sns-output");
    let args = [
        "moments",
        "export-snapshot",
        database.to_str().unwrap(),
        output.to_str().unwrap(),
        "--contacts",
        "synthetic",
        "--utc-offset",
        "+08:00",
    ];
    let result = success(fixture.run(&profile, &args));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["posts"], 1);
    assert_eq!(report["invalid"], 0);
    let timeline_path = output.join("synthetic/SNS/timeline.json");
    let timeline_bytes = fs::read(&timeline_path).unwrap();
    let timeline: serde_json::Value = serde_json::from_slice(&timeline_bytes).unwrap();
    assert_eq!(
        timeline["posts"][0]["content_desc"],
        "synthetic offline post"
    );
    let html = fs::read_to_string(output.join("synthetic/SNS/timeline.html")).unwrap();
    assert!(html.contains("Content-Security-Policy"));
    assert!(!fixture.run(&profile, &args).status.success());
    assert_eq!(fs::read(&timeline_path).unwrap(), timeline_bytes);
    assert_eq!(fs::read(&database).unwrap(), before);
    assert_eq!(fs::read_dir(output.join("synthetic")).unwrap().count(), 1);
}

#[test]
#[ignore = "requires native ffmpeg in PATH; explicitly run with audio integration"]
fn native_audio_cli_works_without_python() {
    let fixture = Fixture::new();
    let profile = fixture.root.join("unconfigured");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/tone.silk");
    let output = fixture.root.join("voice.mp3");
    let before = fs::read(&source).unwrap();
    fs::write(&output, b"previous result").unwrap();
    let result = success(fixture.run(
        &profile,
        &[
            "audio",
            "convert",
            source.to_str().unwrap(),
            output.to_str().unwrap(),
        ],
    ));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        report["size"].as_u64().unwrap(),
        fs::metadata(&output).unwrap().len()
    );
    assert!(report["size"].as_u64().unwrap() > 0);
    assert_eq!(fs::read(source).unwrap(), before);
    let mut entries: Vec<_> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(entries, ["shared-runtime", "voice.mp3"]);
    assert!(fixture.root.join("shared-runtime/bootstrap").is_dir());
    assert!(!profile.exists());
}

#[test]
fn sns_cache_cli_publishes_consistent_media_references() {
    use base64::Engine;
    let fixture = Fixture::new();
    let profile = fixture.root.join("unconfigured");
    let source = fixture.root.join("source");
    fs::create_dir(&source).unwrap();
    let database = source.join("sns.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); CREATE TABLE SnsMessage_tmp3(feed_id INTEGER, create_time INTEGER,type INTEGER,from_username TEXT,from_nickname TEXT,to_username TEXT,to_nickname TEXT,content TEXT,del_status INTEGER);").unwrap();
    conn.execute("INSERT INTO SnsTimeLine VALUES (1,'synthetic',?1)", ["<x><TimelineObject><id>1</id><username>synthetic</username><createTime>1710000000</createTime><contentDesc>synthetic cache post</contentDesc><ContentObject><contentStyle>1</contentStyle><mediaList><media><id>image1</id><type>2</type><url>https://example.invalid/image</url><size width=\"1\" height=\"1\"/></media><media><id>video1</id><type>6</type><url>https://example.invalid/video</url></media></mediaList></ContentObject></TimelineObject></x>"]).unwrap();
    drop(conn);
    let original_db = fs::read(&database).unwrap();
    let cache = fixture.root.join("cache");
    let image = cache.join("2024-03/Sns/Img/aa/image.dat");
    fs::create_dir_all(image.parent().unwrap()).unwrap();
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aC1sAAAAASUVORK5CYII=").unwrap();
    let encrypted: Vec<u8> = png.iter().map(|byte| byte ^ 0x37).collect();
    fs::write(&image, &encrypted).unwrap();
    let key = format!("{:x}", md5::compute("1_video1_3"));
    let video = cache
        .join("2024-03/Sns/Video")
        .join(&key[..2])
        .join(format!("{}.mp4", &key[2..]));
    fs::create_dir_all(video.parent().unwrap()).unwrap();
    // 仅测试缓存拷贝契约；该合成 ftyp 头不用于证明视频解码或播放能力。
    let mp4 = b"\x00\x00\x00\x18ftypisom\x00\x00\x00\x00isommp42";
    fs::write(&video, mp4).unwrap();
    let output = fixture.root.join("sns-cached");
    let args = [
        "moments",
        "export-snapshot",
        database.to_str().unwrap(),
        output.to_str().unwrap(),
        "--xwechat-cache",
        cache.to_str().unwrap(),
        "--contacts",
        "synthetic",
        "--utc-offset",
        "+08:00",
    ];
    let result = success(fixture.run(&profile, &args));
    let report: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(report["media_recovered"], 2);
    assert_eq!(report["media_failed"], 0);
    assert_eq!(report["media_missing"], 0);
    let sns = output.join("synthetic/SNS");
    let timeline: serde_json::Value =
        serde_json::from_slice(&fs::read(sns.join("timeline.json")).unwrap()).unwrap();
    let post = &timeline["posts"][0];
    let image_ref = post["media"][0]["local_file"].as_str().unwrap();
    let video_ref = post["media"][1]["local_file"].as_str().unwrap();
    assert_eq!(post["media"][1]["id"], "video1");
    assert_eq!(post["media"][1]["video_complete"], true);
    assert_eq!(fs::read(sns.join(image_ref)).unwrap(), png);
    assert_eq!(fs::read(sns.join(video_ref)).unwrap(), mp4);
    let recovery: serde_json::Value =
        serde_json::from_slice(&fs::read(sns.join("_media_recovery.json")).unwrap()).unwrap();
    let single: serde_json::Value = serde_json::from_slice(
        &fs::read(sns.join(recovery[0]["post_file"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(&single, post);
    let html = fs::read_to_string(sns.join("timeline.html")).unwrap();
    assert!(html.contains(&format!("src=\"{image_ref}\"")));
    assert!(html.contains(&format!("src=\"{video_ref}\"")));
    assert!(!html.contains("src=\"https:"));
    assert!(!fixture.run(&profile, &args).status.success());
    assert_eq!(fs::read(&database).unwrap(), original_db);
    assert_eq!(fs::read(&image).unwrap(), encrypted);
    assert_eq!(fs::read(&video).unwrap(), mp4);
}

#[test]
fn accounts_have_independent_processes_and_concurrent_start_is_singleton() {
    let mut fixture = Fixture::new();
    let a = fixture.account("alpha-person", true);
    let b = fixture.account("beta-person", true);
    let accounts = fixture.root.join("shared-runtime/accounts");
    assert!(!accounts.exists(), "Seeding keys must not start a daemon");
    let stores = [&a, &b].map(|profile| {
        let path = profile.join("keys.dpapi");
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"WXKEYS\0\x01"));
        (path, bytes)
    });
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let root = fixture.root.clone();
            let profile = a.clone();
            std::thread::spawn(move || success(run(&root, &profile, &["contacts", "--json"])))
        })
        .collect();
    for thread in threads {
        let text = thread.join().unwrap();
        assert!(text.contains("alpha-person"));
        assert!(!text.contains("beta-person"));
    }
    let text = success(fixture.run(&b, &["contacts", "--json"]));
    assert!(text.contains("beta-person") && !text.contains("alpha-person"));
    let records: Vec<serde_json::Value> = fs::read_dir(&accounts)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&fs::read(entry.unwrap().path().join("daemon.pid")).unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["pid"], records[1]["pid"]);
    assert_ne!(records[0]["runtime_id"], records[1]["runtime_id"]);
    for entry in fs::read_dir(&accounts).unwrap() {
        let path = entry.unwrap().path();
        let log = fs::read_to_string(path.join("daemon.log")).unwrap();
        assert_eq!(log.matches("[daemon] wx-daemon 启动").count(), 1);
    }
    let before = success(fixture.run(&b, &["daemon", "status"]));
    success(fixture.run(&a, &["daemon", "stop"]));
    let after = success(fixture.run(&b, &["daemon", "status"]));
    assert_eq!(before, after);
    assert!(after.contains("运行中"));
    assert!(success(fixture.run(&a, &["daemon", "status"])).contains("未运行"));
    assert!(success(fixture.run(&b, &["contacts", "--json"])).contains("beta-person"));
    for (path, bytes) in stores {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn failed_query_keeps_service_alive_and_retries_after_keys_are_repaired() {
    let mut fixture = Fixture::new();
    let bad = fixture.account("broken-account", false);
    let output = fixture.run(&bad, &["contacts", "--json"]);
    assert!(!output.status.success());
    assert!(success(fixture.run(&bad, &["daemon", "status"])).contains("运行中"));
    let directory = fs::read_dir(fixture.root.join("shared-runtime/accounts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record = fs::read(directory.join("daemon.pid")).unwrap();
    let info: serde_json::Value =
        serde_json::from_str(&success(fixture.run(&bad, &["tasks", "info"]))).unwrap();
    assert_eq!(info["configured"], false);
    let keys = serde_json::json!({"contact/contact.db": "11".repeat(32)});
    fs::write(bad.join("all_keys.json"), keys.to_string()).unwrap();
    fixture.seed_keys(&bad, &keys);
    assert!(success(fixture.run(&bad, &["contacts", "--json"])).contains("broken-account"));
    assert_eq!(fs::read(directory.join("daemon.pid")).unwrap(), record);
    success(fixture.run(&bad, &["daemon", "stop"]));
    for entry in fs::read_dir(fixture.root.join("shared-runtime/accounts")).unwrap() {
        assert!(!entry.unwrap().path().join("daemon.pid").exists());
    }
}

#[test]
fn custom_home_discovers_its_own_config_without_explicit_config_override() {
    let mut fixture = Fixture::new();
    let profile = fixture.account("home-person", true);
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_wx"))
            .args(args)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WX_CLI_CONFIG")
            .env("WX_CLI_HOME", &profile)
            .current_dir(&fixture.root)
            .output()
            .unwrap()
    };
    let output = call(&["contacts", "--json"]);
    let stopped = call(&["daemon", "stop"]);
    assert!(success(output).contains("home-person"));
    success(stopped);
    let empty = Command::new(env!("CARGO_BIN_EXE_wx"))
        .args(["contacts", "--json"])
        .env_remove("WX_DAEMON_MODE")
        .env("WX_CLI_CONFIG", "")
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(!empty.status.success());
}

#[test]
fn tampered_birth_time_is_rejected_and_stale_record_allows_restart() {
    let mut fixture = Fixture::new();
    let account = fixture.account("identity-person", true);
    eprintln!("阶段 1：初次 contacts 启动后台");
    success(fixture.run(&account, &["contacts", "--json"]));
    let directory = fs::read_dir(fixture.root.join("shared-runtime/accounts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let pid_path = directory.join("daemon.pid");
    let original = fs::read(&pid_path).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
    // 保留退出进程的内核对象，确定性覆盖 OpenProcess 成功但进程已退出的状态。
    struct HeldProcess(windows::Win32::Foundation::HANDLE);
    impl Drop for HeldProcess {
        fn drop(&mut self) {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
        }
    }
    let _held_process = HeldProcess(unsafe {
        windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            record["pid"].as_u64().unwrap().try_into().unwrap(),
        )
        .unwrap()
    });
    record["created"] = serde_json::json!(record["created"].as_u64().unwrap() + 1);
    let tampered = record.to_string().into_bytes();
    fs::write(&pid_path, &tampered).unwrap();
    eprintln!("阶段 2：篡改创建时间后拒绝 stop");
    let rejected = fixture.run(&account, &["daemon", "stop"]);
    let retained = fs::read(&pid_path);
    // 无论断言是否通过，都先恢复测试账号的合法记录，确保析构可停止后台。
    fs::write(&pid_path, &original).unwrap();
    assert!(!rejected.status.success());
    assert_eq!(
        retained.unwrap(),
        tampered,
        "identity rejection must preserve the PID record"
    );
    eprintln!("阶段 3：恢复合法 PID 记录后查询 status");
    assert!(success(fixture.run(&account, &["daemon", "status"])).contains("运行中"));
    eprintln!("阶段 4：停止合法后台");
    success(fixture.run(&account, &["daemon", "stop"]));
    fs::write(&pid_path, &original).unwrap();
    eprintln!("阶段 4b：写回陈旧 PID 记录后 stop 清除记录");
    success(fixture.run(&account, &["daemon", "stop"]));
    assert!(!pid_path.exists(), "陈旧 PID 记录应由 stop 清除");
    fs::write(&pid_path, &original).unwrap();
    eprintln!("阶段 5：写回陈旧 PID 记录后 contacts 重启");
    assert!(success(fixture.run(&account, &["contacts", "--json"])).contains("identity-person"));
    assert_ne!(fs::read(&pid_path).unwrap(), original);
}

// 测试数据使用固定虚构密钥；生产代码不提供数据库加密写回入口。
#[test]
fn export_keeps_null_timestamp_rows_and_empty_tables() {
    let mut fixture = Fixture::new();
    for with_row in [true, false] {
        let name = if with_row {
            "null-time-person"
        } else {
            "empty-person"
        };
        let profile = fixture.account(name, true);
        let plain = profile.join("message-plain.db");
        fs::copy(profile.join("fixture.db"), &plain).unwrap();
        let conn = rusqlite::Connection::open(&plain).unwrap();
        let table = format!("Msg_{:x}", md5::compute(name.as_bytes()));
        conn.execute_batch(&format!("CREATE TABLE [{table}] (local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")).unwrap();
        if with_row {
            conn.execute(
                &format!("INSERT INTO [{table}] VALUES (1,1,NULL,NULL,'preserved',0)"),
                [],
            )
            .unwrap();
        }
        drop(conn);
        fs::create_dir_all(profile.join("db_storage/message")).unwrap();
        encrypt_fixture(&plain, &profile.join("db_storage/message/message_0.db"));
        let mut keys: serde_json::Value =
            serde_json::from_slice(&fs::read(profile.join("all_keys.json")).unwrap()).unwrap();
        keys["message/message_0.db"] = serde_json::json!("11".repeat(32));
        fs::write(profile.join("all_keys.json"), keys.to_string()).unwrap();
        fixture.seed_keys(&profile, &keys);
        let output = fixture.root.join(format!("{name}.json"));
        success(fixture.run(&profile, &["export-chat", name, output.to_str().unwrap()]));
        let value: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(
            value["messages"].as_array().unwrap().len(),
            usize::from(with_row)
        );
        if with_row {
            assert!(value["messages"][0]["timestamp"].is_null());
            assert_eq!(value["messages"][0]["content"], "preserved");
        }
    }
}

// 测试数据使用固定虚构密钥；生产代码不提供数据库加密写回入口。
fn encrypt_fixture(plain: &Path, output: &Path) {
    let plain = fs::read(plain).unwrap();
    assert_eq!(plain[20], 80);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
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

#[test]
fn detail_clis_preserve_ambiguity_exit_code_and_select_exact_shards() {
    let mut fixture = Fixture::new();
    let profile = fixture.account("transfer-person", true);
    let mut keys: serde_json::Value =
        serde_json::from_slice(&fs::read(profile.join("all_keys.json")).unwrap()).unwrap();
    fs::create_dir_all(profile.join("db_storage/message")).unwrap();
    let table = format!("Msg_{:x}", md5::compute(b"transfer-person"));
    for index in 0..2 {
        let plain = profile.join(format!("message-{index}.db"));
        fs::copy(profile.join("fixture.db"), &plain).unwrap();
        let conn = rusqlite::Connection::open(&plain).unwrap();
        conn.execute_batch(&format!("CREATE TABLE [{table}] (local_id INTEGER, local_type INTEGER, create_time INTEGER, message_content BLOB, WCDB_CT_message_content INTEGER)")).unwrap();
        let xml = format!("<msg><appmsg><type>2000</type><title>微信转账</title><wcpayinfo><paysubtype>3</paysubtype><feedesc>¥{index}.01</feedesc></wcpayinfo></appmsg></msg>");
        let bytes = zstd::encode_all(xml.as_bytes(), 1).unwrap();
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES (7, ?1, ?2, ?3, 4)"),
            rusqlite::params![(1i64 << 32) | 49, 100 + index, bytes],
        )
        .unwrap();
        let location = format!(
            "<msg><location poiname='示例地点{index}' poiPhone='000' x='31.2' y='121.5'/></msg>"
        );
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES (8, ?1, ?2, ?3, 4)"),
            rusqlite::params![
                (1i64 << 32) | 48,
                100 + index,
                zstd::encode_all(location.as_bytes(), 1).unwrap()
            ],
        )
        .unwrap();
        conn.execute_batch(&format!("ALTER TABLE [{table}] ADD COLUMN real_sender_id INTEGER; CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('transfer-person'); UPDATE [{table}] SET real_sender_id=1;")).unwrap();
        drop(conn);
        let rel = format!("message/message_{index}.db");
        encrypt_fixture(&plain, &profile.join("db_storage").join(&rel));
        keys[rel] = serde_json::json!("11".repeat(32));
    }
    fs::write(profile.join("all_keys.json"), keys.to_string()).unwrap();
    fixture.seed_keys(&profile, &keys);
    let export_path = fixture.root.join("native-export.json");
    success(fixture.run(
        &profile,
        &[
            "export-chat",
            "transfer-person",
            export_path.to_str().unwrap(),
        ],
    ));
    let exported: serde_json::Value =
        serde_json::from_slice(&fs::read(&export_path).unwrap()).unwrap();
    let messages = exported["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["timestamp"], 100);
    assert_eq!(messages[3]["timestamp"], 101);
    assert_eq!(messages[0]["sender"], "transfer-person");
    assert_eq!(messages[0]["source"], "message/message_0.db");
    assert_eq!(messages[0]["type"], "transfer");
    assert_eq!(messages[0]["transfer"]["fee_desc"], "¥0.01");
    assert_eq!(messages[0]["transfer"]["direction"], "已收款");
    let previous = fs::read(&export_path).unwrap();
    assert!(!fixture
        .run(
            &profile,
            &[
                "export-chat",
                "missing-person",
                export_path.to_str().unwrap()
            ]
        )
        .status
        .success());
    assert_eq!(fs::read(&export_path).unwrap(), previous);
    let forbidden = profile.join("db_storage/export.json");
    assert!(!fixture
        .run(
            &profile,
            &[
                "export-chat",
                "transfer-person",
                forbidden.to_str().unwrap()
            ]
        )
        .status
        .success());
    assert!(!forbidden.exists());
    for name in ["all_keys.json", "keys.dpapi", "config.json"] {
        let key_path = profile.join(name);
        let keys_before = fs::read(&key_path).unwrap();
        assert!(!fixture
            .run(
                &profile,
                &["export-chat", "transfer-person", key_path.to_str().unwrap()]
            )
            .status
            .success());
        assert_eq!(fs::read(&key_path).unwrap(), keys_before);
    }
    let ambiguous = fixture.run(&profile, &["decode-transfer", "transfer-person", "7"]);
    assert_eq!(
        ambiguous.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&ambiguous.stderr)
    );
    assert!(
        String::from_utf8_lossy(&ambiguous.stderr).contains("Business request refused"),
        "status={}\nstdout={}\nstderr={}",
        ambiguous.status,
        String::from_utf8_lossy(&ambiguous.stdout),
        String::from_utf8_lossy(&ambiguous.stderr),
    );
    let selected = success(fixture.run(
        &profile,
        &["decode-transfer", "transfer-person", "7", "101", "--json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&selected).unwrap();
    assert_eq!(value["transfer"]["fee_desc"], "¥1.01");
    assert_eq!(value["create_time"], 101);
    let missing = fixture.run(
        &profile,
        &["decode-transfer", "transfer-person", "999", "--json"],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&missing.stdout).unwrap()["exit_code"],
        1
    );
    let ambiguous = fixture.run(
        &profile,
        &["decode-location", "transfer-person", "8", "--json"],
    );
    assert_eq!(ambiguous.status.code(), Some(2));
    let selected = success(fixture.run(
        &profile,
        &["decode-location", "transfer-person", "8", "101", "--json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&selected).unwrap();
    assert_eq!(value["location"]["poiname"], "示例地点1");
    assert_eq!(value["location"]["lng"], 121.5);
    assert_eq!(value["source"], "message_1.db");
    let text = success(fixture.run(
        &profile,
        &["decode-location", "transfer-person", "8", "100"],
    ));
    assert!(text.contains("POI 名: 示例地点0"));
    let wrong_type = fixture.run(
        &profile,
        &["decode-location", "transfer-person", "7", "100", "--json"],
    );
    assert_eq!(wrong_type.status.code(), Some(1));
}
