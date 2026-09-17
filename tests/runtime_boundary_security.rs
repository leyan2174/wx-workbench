//! 运行边界安全契约：仅合成 SQLite，禁止依赖真实账号、Python 或 ffmpeg。
use rusqlite::Connection;
#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "support/cli_output.rs"]
mod cli_output;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(args)
            .current_dir(self.0.path())
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_CONTACTS")
            .env_remove("WECHAT_EXPORT_USERS")
            .env("WX_CLI_CONFIG", self.path("ambient.json"))
            .env("WX_CLI_HOME", self.path("runtime"))
            .env("PATH", "");
        let output = cli_output::output(&mut command, self.0.path(), Duration::from_secs(60));
        assert_eq!(
            fs::read(self.path("ambient.json")).unwrap(),
            b"invalid ambient account config"
        );
        bootstrap::assert_only_bootstrap(&self.path("runtime"));
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(bootstrap::BootstrapCleanup(self.path("runtime")));
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
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

#[test]
fn removed_audio_export_preserves_existing_files_without_tools() {
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
    let error = failure(f.run(&[
        "audio",
        "export",
        "--config",
        arg(&config),
        "--contacts",
        "alice",
    ]));
    assert!(error.contains("unrecognized subcommand 'audio'"));
    {
        let target = config_dir.join("decrypted/new");
        failure(f.run(&[
            "audio",
            "export",
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
        let mut args = vec!["moments", "export-snapshot", arg(&sns), arg(&output)];
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
            "moments",
            "export-snapshot",
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
        "moments",
        "export-snapshot",
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
fn removed_audio_export_rejects_paths_before_any_directory_creation() {
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
        "audio",
        "export",
        "--config",
        arg(&config),
        "--output-dir",
        arg(&traversal),
    ]));
    assert_eq!(fs::read(&media).unwrap(), original);
    // Unsupported commands cannot create output, even with explicit source and target paths.
    assert!(
        !f.path("escaped").exists(),
        "父目录穿越已经产生写入，stderr/stdout: {error}"
    );
    assert!(
        error.contains("unrecognized subcommand 'audio'"),
        "unsupported command must fail before account access: {error}"
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
            "chats",
            "export",
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
