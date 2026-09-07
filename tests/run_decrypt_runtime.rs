#![cfg(windows)]

#[path = "fixtures/mcp-readonly-runtime/encrypted_sqlite.rs"]
mod encrypted_sqlite;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime},
};

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    keys: PathBuf,
    source: PathBuf,
    destination: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        let source = account.join("db_storage");
        fs::create_dir_all(&source).unwrap();
        let fixture = Self {
            config: account.join("config.db"),
            keys: account.join("all_keys.db"),
            destination: account.join("decrypted"),
            source,
            root,
        };
        // 只匹配当前合成测试进程，不匹配真实微信进程。
        let process = std::env::current_exe().unwrap();
        fs::write(
            &fixture.config,
            serde_json::to_vec(&json!({
                "db_dir": "db_storage",
                "keys_file": "all_keys.db",
                "decrypted_dir": "decrypted",
                "wechat_process": process.file_name().unwrap().to_str().unwrap()
            }))
            .unwrap(),
        )
        .unwrap();
        fixture.write_keys(json!({"unrelated/missing.db": "11".repeat(32)}));
        fixture
    }

    fn write_keys(&self, keys: Value) {
        fs::write(&self.keys, serde_json::to_vec(&keys).unwrap()).unwrap();
    }

    fn configure(&self, field: &str, value: &str) {
        let mut config: Value = serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap();
        config[field] = value.into();
        fs::write(&self.config, serde_json::to_vec(&config).unwrap()).unwrap();
    }

    fn database(&self, name: &str, marker: &str) {
        let output = self.source.join(name);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        let plain = self.root.path().join("synthetic-plain.db");
        let db = encrypted_sqlite::sqlite(&plain);
        db.execute_batch("CREATE TABLE marker(value TEXT NOT NULL)")
            .unwrap();
        db.execute("INSERT INTO marker VALUES (?1)", [marker])
            .unwrap();
        drop(db);
        encrypted_sqlite::encrypt(&plain, &output);
    }

    fn command(&self, direct: bool, args: &[&str]) -> Output {
        let mut command = command(self.root.path(), &self.config);
        if direct {
            command.args(["toolkit", "decrypt"]);
        } else {
            command.args(["toolkit", "run", "decrypt", "--"]);
        }
        command.args(args).output().unwrap()
    }

    fn protected(&self) -> Vec<Vec<u8>> {
        [&self.config, &self.keys]
            .map(|path| fs::read(path).unwrap())
            .into()
    }
}

fn command(root: &Path, config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_CONFIG", config)
        .env("WX_CLI_HOME", root.join("isolated-runtime"))
        .env("WX_WECHAT_DECRYPT_PYTHON", root.join("missing-python.exe"))
        .env("PATH", "")
        .current_dir(root);
    command
}

fn diagnostic(output: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn success(output: &Output) {
    assert!(output.status.success(), "{}", diagnostic(output));
}

fn count(output: &Output, fields: &[&str], chinese: &str, expected: usize) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(report) = serde_json::from_str::<Value>(&stdout) {
        let actual = fields.iter().find_map(|field| {
            report.get(*field).and_then(|value| {
                value
                    .as_u64()
                    .map(|n| n as usize)
                    .or_else(|| value.as_array().map(Vec::len))
            })
        });
        assert_eq!(actual, Some(expected), "{}", diagnostic(output));
    } else {
        let pattern = format!(r"(\d+)\s+{}", chinese);
        let expression = regex::Regex::new(&pattern).unwrap();
        let actual = expression
            .captures(&stdout)
            .and_then(|capture| capture[1].parse::<usize>().ok());
        assert_eq!(actual, Some(expected), "{}", diagnostic(output));
    }
}

// 同时记录目录项、字节和修改时间，确保预览不会悄悄重写文件。
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)> {
    fn visit(
        root: &Path,
        path: &Path,
        result: &mut BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime)>,
    ) {
        let metadata = fs::metadata(path).unwrap();
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (
                metadata.is_file().then(|| fs::read(path).unwrap()),
                metadata.modified().unwrap(),
            ),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut result);
    }
    result
}

fn marker(path: &Path) -> String {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        db.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    db.query_row("SELECT value FROM marker", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn run_decrypt_native_batch_reuses_keys_and_retains_failed_output() {
    let f = Fixture::new();
    for (name, value) in [
        ("a/valid.db", "first"),
        ("b/valid.db", "second"),
        ("missing.db", "no-key"),
        ("bad.db", "bad-hmac"),
    ] {
        f.database(name, value);
    }
    let mut damaged = fs::read(f.source.join("bad.db")).unwrap();
    damaged[4032] ^= 1;
    fs::write(f.source.join("bad.db"), damaged).unwrap();
    f.write_keys(json!({
        "a\\valid.db": {"enc_key": "11".repeat(32)},
        "b/valid.db": "11".repeat(32),
        "bad.db": "11".repeat(32),
        "unrelated/missing.db": "11".repeat(32),
        "_db_dir": ""
    }));
    fs::create_dir(&f.destination).unwrap();
    fs::write(f.destination.join("bad.db"), b"old target must survive").unwrap();
    let protected = f.protected();
    let source = snapshot(&f.source);
    let output = f.command(false, &[]);
    success(&output);
    count(&output, &["written", "success", "succeeded"], "成功", 2);
    count(&output, &["failures", "failed"], "失败", 1);
    count(&output, &["skipped"], "跳过", 1);
    count(
        &output,
        &["no_key", "missing_keys", "missing_key"],
        r"跳过\(无密钥\)",
        1,
    );
    assert_eq!(marker(&f.destination.join("a/valid.db")), "first");
    assert_eq!(marker(&f.destination.join("b/valid.db")), "second");
    assert!(!f.destination.join("missing.db").exists());
    assert_eq!(
        fs::read(f.destination.join("bad.db")).unwrap(),
        b"old target must survive"
    );
    assert_eq!(f.protected(), protected);
    assert_eq!(snapshot(&f.source), source);
}

#[test]
fn direct_decrypt_needs_no_process_and_returns_nonzero_for_item_failure() {
    let f = Fixture::new();
    f.configure(
        "wechat_process",
        "wx-synthetic-process-that-does-not-exist.exe",
    );
    f.database("valid.db", "offline");
    f.write_keys(json!({"valid.db": "11".repeat(32)}));
    let protected = f.protected();
    success(&f.command(true, &[]));
    assert_eq!(marker(&f.destination.join("valid.db")), "offline");
    f.database("missing.db", "no-key");
    let output = f.command(true, &[]);
    assert!(!output.status.success(), "{}", diagnostic(&output));
    count(&output, &["failures", "failed"], "失败", 1);
    assert!(!f.destination.join("missing.db").exists());
    assert_eq!(f.protected(), protected);
}

#[test]
fn incremental_aliases_skip_before_parsing_key_hex() {
    for flag in ["-i", "--incremental"] {
        let f = Fixture::new();
        f.database("valid.db", "source");
        f.write_keys(json!({"valid.db": "not-hex"}));
        fs::create_dir(&f.destination).unwrap();
        let destination = f.destination.join("valid.db");
        fs::write(&destination, b"newer target must not be opened").unwrap();
        let source_time = fs::metadata(f.source.join("valid.db"))
            .unwrap()
            .modified()
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&destination)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(source_time + Duration::from_secs(60)))
            .unwrap();
        let before = snapshot(&f.destination);
        let protected = f.protected();
        let output = f.command(false, &[flag]);
        success(&output);
        count(&output, &["unchanged", "skipped"], "未变更", 1);
        count(&output, &["failures", "failed"], "失败", 0);
        assert_eq!(snapshot(&f.destination), before);
        assert_eq!(f.protected(), protected);
    }
}

#[test]
fn dry_run_preserves_source_targets_and_does_not_create_output_directory() {
    for existing_output in [false, true] {
        let f = Fixture::new();
        f.database("valid.db", "preview");
        f.database("missing.db", "no-key");
        f.write_keys(json!({"valid.db": "11".repeat(32)}));
        if existing_output {
            fs::create_dir(&f.destination).unwrap();
            fs::write(f.destination.join("valid.db"), b"existing output").unwrap();
        }
        let source = snapshot(&f.source);
        let destination = snapshot(&f.destination);
        let protected = f.protected();
        let output = f.command(false, &["--dry-run"]);
        success(&output);
        count(
            &output,
            &["no_key", "missing_keys", "missing_key"],
            r"跳过\(无密钥\)",
            1,
        );
        count(&output, &["failures", "failed"], "失败", 0);
        assert_eq!(snapshot(&f.source), source);
        assert_eq!(snapshot(&f.destination), destination);
        assert_eq!(f.destination.exists(), existing_output);
        assert_eq!(f.protected(), protected);
    }
}

#[test]
fn output_sidecars_are_rejected_and_never_removed() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let f = Fixture::new();
        f.database("valid.db", "replacement");
        f.write_keys(json!({"valid.db": "11".repeat(32)}));
        fs::create_dir(&f.destination).unwrap();
        fs::write(f.destination.join("valid.db"), b"old database").unwrap();
        fs::write(
            f.destination.join(format!("valid.db{suffix}")),
            b"active sidecar",
        )
        .unwrap();
        let before = snapshot(&f.destination);
        let output = f.command(false, &[]);
        success(&output);
        assert!(
            snapshot(&f.destination) == before,
            "sidecar {suffix}: {}",
            diagnostic(&output)
        );
        count(&output, &["failures", "failed"], "失败", 1);
    }
}

#[test]
fn raw_key_slash_and_case_collisions_are_rejected_before_output() {
    for alias in ["nested\\valid.db", "NESTED/VALID.DB"] {
        let f = Fixture::new();
        f.database("nested/valid.db", "collision");
        let mut keys = json!({"nested/valid.db": "11".repeat(32)});
        keys[alias] = json!("11".repeat(32));
        f.write_keys(keys);
        let protected = f.protected();
        let source = snapshot(&f.source);
        let output = f.command(false, &[]);
        assert!(!output.status.success(), "{}", diagnostic(&output));
        assert!(!f.destination.exists());
        assert_eq!(f.protected(), protected);
        assert_eq!(snapshot(&f.source), source);
    }
}

#[test]
fn output_cannot_overwrite_config_or_saved_keys() {
    for name in ["config.db", "all_keys.db"] {
        let f = Fixture::new();
        f.database(name, "must not replace account metadata");
        let mut keys = serde_json::Map::new();
        keys.insert(name.into(), json!("11".repeat(32)));
        f.write_keys(Value::Object(keys));
        f.configure("decrypted_dir", ".");
        let protected = f.protected();
        let source = snapshot(&f.source);
        let output = f.command(false, &[]);
        assert_eq!(f.protected(), protected, "{}", diagnostic(&output));
        assert_eq!(snapshot(&f.source), source);
        // 可在全局预检拒绝，也可作为明确的逐项失败；两种情况均不得修改源文件。
        if output.status.success() {
            count(&output, &["failures", "failed"], "失败", 1);
        }
    }
}

#[test]
fn run_decrypt_help_requires_neither_config_nor_python() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["toolkit", "run", "decrypt", "--", "--help"],
        vec!["toolkit", "run", "decrypt", "--", "-h"],
    ] {
        let output = command(root.path(), &root.path().join("absent/config.json"))
            .args(args)
            .output()
            .unwrap();
        success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--incremental"), "{}", diagnostic(&output));
        assert!(stdout.contains("--dry-run"), "{}", diagnostic(&output));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn legacy_run_includes_migrate_while_direct_decrypt_skips_it() {
    for direct in [false, true] {
        let f = Fixture::new();
        f.database("valid.db", "ordinary");
        f.database("migrate/old.db", "migration");
        f.write_keys(json!({
            "valid.db": "11".repeat(32),
            "migrate/old.db": "11".repeat(32)
        }));
        let output = f.command(direct, &[]);
        success(&output);
        assert_eq!(marker(&f.destination.join("valid.db")), "ordinary");
        assert_eq!(f.destination.join("migrate/old.db").exists(), !direct);
        if !direct {
            assert_eq!(marker(&f.destination.join("migrate/old.db")), "migration");
        }
    }
}

#[test]
fn dry_run_plans_before_parsing_key_hex() {
    let f = Fixture::new();
    f.database("valid.db", "preview without key parsing");
    f.write_keys(json!({"valid.db": "not-hex"}));
    let source = snapshot(&f.source);
    let protected = f.protected();
    let output = f.command(false, &["--dry-run"]);
    success(&output);
    count(&output, &["planned"], "待解密", 1);
    count(&output, &["failures", "failed"], "失败", 0);
    assert!(!f.destination.exists());
    assert_eq!(snapshot(&f.source), source);
    assert_eq!(f.protected(), protected);
}
