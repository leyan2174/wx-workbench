//! 完整 wx 子进程离线计划回归；仅合成 SQLite、元数据和媒体，无 Python 依赖。
use chrono::{Local, TimeZone};
#[path = "support/bootstrap.rs"]
mod bootstrap;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    source: PathBuf,
    metadata: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let cache = root.join("decrypted-cache");
        let source = root.join("synthetic-source");
        let metadata = root.join("metadata/chats.json");
        for dir in [
            &cache,
            &source,
            metadata.parent().unwrap(),
            &root.join("empty-path"),
            &root.join("home"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(
            &metadata,
            include_bytes!("fixtures/chat-plan-runtime/metadata.json"),
        )
        .unwrap();
        fs::write(
            root.join("must-not-read-config.json"),
            b"invalid synthetic config; must not be read",
        )
        .unwrap();
        let table = |user: &str| format!("Msg_{:x}", md5::compute(user));
        let conn = Connection::open(cache.join("messages.db")).unwrap();
        for user in ["alpha", "beta"] {
            conn.execute_batch(&format!("CREATE TABLE [{}](create_time INTEGER, message_content, compress_content, packed_info_data);", table(user))).unwrap();
        }
        conn.execute_batch(&format!("INSERT INTO [{}] VALUES (NULL,NULL,NULL,NULL),(99,'before',NULL,NULL),(100,'中文',x'010203',NULL),(101,NULL,NULL,NULL),(102,'abc',NULL,x'00ff'),(103,'after',NULL,NULL);", table("alpha"))).unwrap();
        drop(conn);
        let conn = Connection::open(cache.join("messages2.db")).unwrap();
        conn.execute_batch(&format!("CREATE TABLE [{}](create_time INTEGER,message_content,compress_content,packed_info_data); INSERT INTO [{}] VALUES (102,'x',NULL,NULL);", table("alpha"), table("alpha"))).unwrap();
        drop(conn);
        let conn = Connection::open(cache.join("resource.db")).unwrap();
        conn.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id VALUES ('alpha'); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_id INTEGER,message_create_time INTEGER); CREATE TABLE MessageResourceDetail(message_id INTEGER,size INTEGER); INSERT INTO MessageResourceInfo VALUES (1,1,100),(1,2,102),(1,3,103),(1,4,101); INSERT INTO MessageResourceDetail VALUES (1,12),(1,NULL),(2,30),(3,900);").unwrap();
        drop(conn);
        let conn = Connection::open(cache.join("media.db")).unwrap();
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('alpha'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,voice_data BLOB); INSERT INTO VoiceInfo VALUES (1,100,x'0102'),(1,102,x'010203'),(1,101,NULL),(1,103,x'00000000');").unwrap();
        drop(conn);
        let hash = format!("{:x}", md5::compute("alpha"));
        for (name, size) in [
            (format!("msg/attach/{hash}/a.bin"), 13),
            (format!("msg/attach/{hash}/nested/copy.bin"), 13),
            (format!("msg/file/{hash}/document"), 7),
            (format!("msg/video/{hash}/clip"), 11),
            (format!("msg/attach/{:x}/a.bin", md5::compute("beta")), 5),
            ("msg/file/unattributed.bin".into(), 1000),
        ] {
            let path = source.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b's'; size]).unwrap();
        }
        fs::hard_link(
            source.join(format!("msg/attach/{hash}/a.bin")),
            source.join(format!("msg/attach/{hash}/hard.bin")),
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            cache,
            source,
            metadata,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wx"));
        cmd.env_clear().current_dir(&self.root);
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.env("PATH", self.root.join("empty-path"))
            .env("HOME", self.root.join("home"))
            .env("USERPROFILE", self.root.join("home"))
            .env("APPDATA", self.root.join("home"))
            .env("LOCALAPPDATA", self.root.join("home"))
            .env("TEMP", &self.root)
            .env("TMP", &self.root)
            .env("WX_CLI_CONFIG", self.root.join("must-not-read-config.json"))
            .env("WX_CLI_HOME", self.root.join("must-not-create-runtime"))
            .env(
                "WX_WECHAT_DECRYPT_PYTHON",
                self.root.join("not-installed-python.exe"),
            )
            .env("PYTHONHOME", self.root.join("not-installed-python"))
            .env("PYTHONPATH", self.root.join("not-installed-python"))
            .env("WECHAT_EXPORT_USERS", "must-not-select-this-user")
            .args(["toolkit", "chat-plan-native"])
            .arg("--decrypted-dir")
            .arg(&self.cache);
        cmd
    }

    fn full_command(&self, output: &Path) -> Command {
        let mut cmd = self.command();
        cmd.args([
            "--message-db",
            "messages.db",
            "--message-db",
            "messages2.db",
            "--resource-db",
            "resource.db",
            "--media-db",
            "media.db",
            "--start",
            "100",
            "--end",
            "102",
        ])
        .arg("--chats-json")
        .arg(&self.metadata)
        .arg("--output")
        .arg(output);
        cmd
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(bootstrap::BootstrapCleanup(
            self.root.join("must-not-create-runtime"),
        ));
    }
}

fn run(mut cmd: Command) -> Output {
    println!("COMMAND: {cmd:?}");
    let output = cmd.output().expect("无法启动 Cargo 构建的 wx 进程");
    println!(
        "EXIT: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "完整 wx 命令失败；主线程需先注册 ChatPlanNative(Args)。stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn failure(output: &Output) {
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty(), "失败必须保留错误说明");
}

fn csv_rows(path: &Path) -> Vec<Vec<String>> {
    let bytes = fs::read(path).unwrap();
    assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(bytes.ends_with(b"\r\n"));
    let mut reader = csv::Reader::from_reader(&bytes[3..]);
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/chat-plan-runtime/expected.json")).unwrap();
    assert_eq!(
        reader.headers().unwrap().iter().collect::<Vec<_>>(),
        expected["header"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    reader
        .records()
        .map(|r| {
            let r = r.unwrap();
            assert_eq!(r.len(), 12);
            r.iter().map(str::to_owned).collect()
        })
        .collect()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    snapshot_except(root, None)
}

fn snapshot_except(root: &Path, excluded: Option<&Path>) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(
        base: &Path,
        path: &Path,
        excluded: Option<&Path>,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if excluded == Some(path.as_path()) {
                continue;
            }
            if path.is_dir() {
                visit(base, &path, excluded, files);
            } else {
                files.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, excluded, &mut files);
    files
}

#[test]
fn estimate_entire_csv_matches_contract_without_python_or_config() {
    let f = Fixture::new();
    let path = f.root.join("estimate.csv");
    let cache = snapshot(&f.cache);
    let source = snapshot(&f.source);
    success(&run(f.full_command(&path)));
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/chat-plan-runtime/expected.json")).unwrap();
    let rows: Vec<Vec<String>> = expected["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.as_array()
                .unwrap()
                .iter()
                .map(|v| match v.as_str().unwrap() {
                    "$START" => Local
                        .timestamp_opt(100, 0)
                        .unwrap()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string(),
                    "$END" => Local
                        .timestamp_opt(102, 0)
                        .unwrap()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string(),
                    s => s.to_owned(),
                })
                .collect()
        })
        .collect();
    assert_eq!(csv_rows(&path), rows);
    assert_eq!(snapshot(&f.cache), cache);
    assert_eq!(snapshot(&f.source), source);
    bootstrap::assert_only_bootstrap(&f.root.join("must-not-create-runtime"));
    assert_eq!(
        fs::read(f.root.join("must-not-read-config.json")).unwrap(),
        b"invalid synthetic config; must not be read"
    );
    assert!(fs::read_dir(f.root.join("empty-path"))
        .unwrap()
        .next()
        .is_none());
    assert!(!f.root.join("not-installed-python.exe").exists());
}

#[test]
fn scan_sizes_hardlinks_filters_and_threads_preserve_csv_order() {
    let f = Fixture::new();
    let before = snapshot(&f.source);
    let mut previous = None;
    for threads in ["1", "6"] {
        let output = f.root.join(format!("scan-{threads}.csv"));
        let mut cmd = f.full_command(&output);
        cmd.args([
            "--size-mode",
            "scan",
            "--threads",
            threads,
            "--user",
            "beta",
            "--user",
            "alpha",
        ])
        .arg("--source-dir")
        .arg(&f.source);
        success(&run(cmd));
        let rows = csv_rows(&output);
        assert_eq!(rows.len(), 2);
        assert_eq!(&rows[0][2], "alpha");
        assert_eq!(&rows[1][2], "beta");
        assert_eq!(&rows[0][9], "57");
        assert_eq!(&rows[1][9], "5");
        assert_eq!(&rows[0][10], "58");
        assert_eq!(&rows[1][11], "partial:scan_limited");
        let bytes = fs::read(output).unwrap();
        if let Some(previous) = previous {
            assert_eq!(bytes, previous);
        }
        previous = Some(bytes);
    }
    let output = f.root.join("media-filter.csv");
    let mut cmd = f.full_command(&output);
    cmd.args([
        "--size-mode",
        "scan",
        "--user",
        "alpha",
        "--user",
        "beta",
        "--exclude-user",
        "beta",
    ])
    .arg("--media-dir")
    .arg(f.source.join("msg"));
    success(&run(cmd));
    let rows = csv_rows(&output);
    assert_eq!(rows.len(), 1);
    assert_eq!(&rows[0][9], "57");
    assert_eq!(snapshot(&f.source), before);
}

#[test]
fn existing_output_is_preserved_and_sources_are_never_destinations() {
    let f = Fixture::new();
    let output = f.root.join("existing.csv");
    fs::write(&output, b"previous bytes").unwrap();
    failure(&run(f.full_command(&output)));
    assert_eq!(fs::read(output).unwrap(), b"previous bytes");
    let before = snapshot(&f.cache);
    let output = f.cache.join("must-not-write.csv");
    failure(&run(f.full_command(&output)));
    assert!(!output.exists());
    assert_eq!(snapshot(&f.cache), before);
    for (flag, root) in [
        ("--source-dir", f.source.clone()),
        ("--media-dir", f.source.join("msg")),
    ] {
        let before = snapshot(&f.source);
        let output = root.join("must-not-write.csv");
        let mut cmd = f.full_command(&output);
        cmd.args(["--size-mode", "scan", flag]).arg(root);
        failure(&run(cmd));
        assert!(!output.exists());
        assert_eq!(snapshot(&f.source), before);
    }
    assert!(
        !snapshot_except(&f.root, Some(&f.root.join("must-not-create-runtime")))
            .keys()
            .any(|p| p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".wx-chat-plan-"))
    );
}

#[test]
fn invalid_arguments_and_unknown_identity_exit_nonzero_without_output() {
    let f = Fixture::new();
    for extra in [
        vec!["--threads", "0"],
        vec!["--threads", "7"],
        vec!["--size-mode", "scan"],
        vec!["--user", "same-name"],
        vec!["--user", "alpha", "--exclude-user", "alpha"],
    ] {
        let output = f.root.join("failure.csv");
        let mut cmd = f.full_command(&output);
        cmd.args(extra);
        failure(&run(cmd));
        assert!(!output.exists());
    }
    let output = f.root.join("no-users.csv");
    let mut cmd = f.command();
    cmd.arg("--output").arg(&output);
    failure(&run(cmd));
    assert!(!output.exists());
    let output = f.root.join("time.csv");
    let mut cmd = f.command();
    cmd.args(["--user", "alpha", "--start", "102", "--end", "100"])
        .arg("--output")
        .arg(&output);
    failure(&run(cmd));
    assert!(!output.exists());
}

#[test]
fn missing_metadata_is_explicit_and_optional_metadata_has_documented_fallback() {
    let f = Fixture::new();
    let output = f.root.join("fallback.csv");
    let mut cmd = f.command();
    cmd.args(["--user", "alpha", "--user", "room@chatroom"])
        .arg("--output")
        .arg(&output);
    success(&run(cmd));
    let rows = csv_rows(&output);
    assert_eq!(&rows[0][1..5], ["1", "alpha", "alpha", "single"]);
    assert_eq!(
        &rows[1][1..5],
        ["2", "room@chatroom", "room@chatroom", "group"]
    );
    assert_eq!(&rows[0][6], "");
    assert_eq!(&rows[0][7], "");
    assert_eq!(&rows[0][9], "");
    assert_eq!(
        &rows[0][11],
        "partial:media_missing,message_db_missing,resource_missing"
    );
    let output = f.root.join("missing-metadata.csv");
    let mut cmd = f.command();
    cmd.arg("--chats-json")
        .arg(f.root.join("does-not-exist.json"))
        .arg("--output")
        .arg(&output);
    failure(&run(cmd));
    assert!(!output.exists());
    let bad = f.root.join("bad.json");
    fs::write(&bad, b"not JSON").unwrap();
    let mut cmd = f.command();
    cmd.arg("--chats-json")
        .arg(bad)
        .arg("--output")
        .arg(&output);
    let result = run(cmd);
    failure(&result);
    assert!(String::from_utf8_lossy(&result.stderr).contains("JSON"));
    assert!(!output.exists());
}

#[test]
fn missing_explicit_databases_produce_visible_partial_csv_not_fake_success() {
    let f = Fixture::new();
    let output = f.root.join("missing-db.csv");
    let mut cmd = f.command();
    cmd.args([
        "--user",
        "alpha",
        "--message-db",
        "missing.db",
        "--resource-db",
        "missing-resource.db",
        "--media-db",
        "missing-media.db",
    ])
    .arg("--output")
    .arg(&output);
    success(&run(cmd));
    let rows = csv_rows(&output);
    assert_eq!(
        &rows[0][11],
        "partial:media_error,message_error,no_message_table,resource_error"
    );
    for name in ["missing.db", "missing-resource.db", "missing-media.db"] {
        assert!(!f.cache.join(name).exists());
    }
}

#[test]
fn estimated_bytes_above_f64_integer_precision_remain_exact() {
    let f = Fixture::new();
    let db = Connection::open(f.cache.join("resource.db")).unwrap();
    db.execute(
        "UPDATE MessageResourceDetail SET size=?1 WHERE message_id=1 AND size IS NOT NULL",
        [9_007_199_254_740_993i64],
    )
    .unwrap();
    drop(db);
    let output = f.root.join("large-integer.csv");
    let mut cmd = f.full_command(&output);
    cmd.args(["--user", "alpha"]);
    success(&run(cmd));
    let rows = csv_rows(&output);
    assert_eq!(&rows[0][8], "9007199254741028");
    assert_eq!(&rows[0][10], "9007199254741039");
}
