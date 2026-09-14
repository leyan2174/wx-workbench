#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "fixtures/mcp-readonly-runtime/encrypted_sqlite.rs"]
mod encrypted_sqlite;

use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const MD5: &str = "0123456789abcdef0123456789abcdef";

fn fixture(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fixture_with_url(root, "")
}

fn fixture_with_url(root: &Path, url: &str) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let source = root.join("db_storage/emoticon/emoticon.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let plain = root.join("plain.db");
    let conn = encrypted_sqlite::sqlite(&plain);
    conn.execute_batch(include_str!("fixtures/emoticons-catalog/schema.sql"))
        .unwrap();
    conn.execute(
        "INSERT INTO kNonStoreEmoticonTable VALUES(?1,'secret-aes',?2,'','pack')",
        [MD5, url],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kStoreEmoticonCaptionsTable VALUES(?1,'Example','default')",
        [MD5],
    )
    .unwrap();
    drop(conn);
    encrypted_sqlite::encrypt(&plain, &source);
    let config = root.join("config.json");
    let keys = root.join("all_keys.json");
    fs::write(&config, serde_json::to_vec(&json!({"db_dir":"db_storage", "keys_file":"all_keys.json", "decrypted_dir":"decrypted"})).unwrap()).unwrap();
    fs::write(
        &keys,
        serde_json::to_vec(&json!({"emoticon/emoticon.db":{"enc_key":"11".repeat(32)},
            "message/old-missing.db":{"enc_key":"22".repeat(32)}}))
        .unwrap(),
    )
    .unwrap();
    [source, config, keys]
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

fn run(root: &Path, args: &[&str]) -> Output {
    command(root)
        .args(["toolkit", "export-emoticons"])
        .args(args)
        .output()
        .unwrap()
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_CONFIG", root.join("config.json"))
        .env("WX_CLI_HOME", root.join("runtime"))
        .env("WX_WECHAT_DECRYPT_PYTHON", root.join("no-python.exe"))
        .env("PATH", "")
        .current_dir(root);
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn encrypted_catalog_preview_and_cache_export_are_native_and_preserve_sources() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    let snapshots = fixture(root.path());
    let preview = success(run(root.path(), &["--dry-run", "--filter", "example"]));
    assert!(preview.contains(MD5) && preview.contains("Example"));
    assert!(!preview.contains("secret-aes"));
    assert!(!root.path().join("exported_emoticons").exists());
    let empty = success(run(root.path(), &["--dry-run", "--filter", "missing"]));
    assert!(empty.contains("共 0 个表情"));
    let output = root.path().join("out");
    fs::create_dir(&output).unwrap();
    let cached = output.join(format!("{MD5}.gif"));
    fs::write(&cached, b"GIF89a-cached").unwrap();
    for _ in 0..2 {
        let report = success(run(root.path(), &["out"]));
        assert!(report.contains("1 成功, 0 失败"));
        assert_eq!(fs::read(&cached).unwrap(), b"GIF89a-cached");
    }
    for (path, bytes) in snapshots {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
}

#[test]
fn unsafe_output_is_rejected_before_creating_nested_database_directory() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    let snapshots = fixture(root.path());
    let result = run(root.path(), &["db_storage/export"]);
    assert!(!result.status.success());
    assert!(!root.path().join("db_storage/export").exists());
    for (path, bytes) in snapshots {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn per_item_failure_retains_legacy_success_exit_and_missing_keys_fail() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    fixture(root.path());
    assert!(success(run(root.path(), &["out"])).contains("0 成功, 1 失败"));
    assert_eq!(fs::read_dir(root.path().join("out")).unwrap().count(), 0);
    fs::remove_file(root.path().join("all_keys.json")).unwrap();
    assert!(!run(root.path(), &["--dry-run"]).status.success());
}

#[test]
fn run_emoticons_help_precedes_configuration_and_process_checks() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wx"))
        .args(["toolkit", "run", "emoticons", "--", "--help"])
        .env_remove("WX_DAEMON_MODE")
        .env("WX_CLI_CONFIG", root.path().join("missing/config.json"))
        .env("WX_CLI_HOME", root.path().join("runtime"))
        .env("PATH", "")
        .current_dir(root.path())
        .output()
        .unwrap();
    let help = success(output);
    assert!(help.contains("--dry-run") && help.contains("--filter"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn encrypted_catalog_downloads_and_publishes_over_loopback_http() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/emoji", listener.local_addr().unwrap());
    let snapshots = fixture_with_url(root.path(), &url);
    let server = std::thread::spawn(move || {
        let start = Instant::now();
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(10) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("loopback test server: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        assert!(String::from_utf8_lossy(&request)
            .to_lowercase()
            .contains("user-agent: mozilla/5.0"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nGIF89a-local",
            )
            .unwrap();
    });
    let result = run(root.path(), &["out"]);
    server.join().unwrap();
    assert!(success(result).contains("1 成功, 0 失败"));
    assert_eq!(
        fs::read(root.path().join(format!("out/{MD5}.gif"))).unwrap(),
        b"GIF89a-local"
    );
    for (path, bytes) in snapshots {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn run_emoticons_reuses_saved_keys_with_a_synthetic_live_process() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    fixture(root.path());
    let config = root.path().join("config.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    // 仅匹配测试进程名称；已有合成密钥应直接复用，绝不读取进程内存。
    settings["wechat_process"] = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
        .into();
    fs::write(&config, serde_json::to_vec(&settings).unwrap()).unwrap();
    let keys = root.path().join("all_keys.json");
    let before = fs::read(&keys).unwrap();
    let output = command(root.path())
        .args(["toolkit", "run", "emoticons", "--", "--dry-run"])
        .output()
        .unwrap();
    assert!(success(output).contains("Example"));
    assert_eq!(fs::read(&keys).unwrap(), before);
    assert!(!root.path().join("exported_emoticons").exists());
}
