use serde_json::Value;
#[path = "support/bootstrap.rs"]
mod bootstrap;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    command(root)
        .args(["progress"])
        .args(args)
        .output()
        .unwrap()
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_CONFIG", root.join("account/config.json"))
        .env("WX_CLI_HOME", root.join("runtime"))
        .env("PATH", "")
        .current_dir(root);
    command
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn progress_missing_config_uses_bootstrap_only() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::BootstrapCleanup(root.path().join("runtime"));
    let status = success(run(root.path(), &["--json"]));
    assert_eq!(status["config_exists"], false);
    assert_eq!(status["databases"]["files"], 0);
    assert!(!root.path().join("account").exists());
    assert!(root.path().join("runtime/bootstrap").is_dir());
    bootstrap::assert_only_bootstrap(&root.path().join("runtime"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn progress_override_counts_without_changing_sources() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    let account = root.path().join("account");
    fs::create_dir(&account).unwrap();
    let config = account.join("config.json");
    let keys = account.join("all_keys.json");
    let exported = root.path().join("chosen");
    fs::create_dir(&exported).unwrap();
    let export = exported.join("chat.json");
    let files = [
        (
            config,
            br#"{"db_dir":"synthetic-db","keys_file":"all_keys.json","decrypted_dir":"decrypted"}"#
                .to_vec(),
        ),
        (keys, b"invalid-json-key-must-not-be-read".to_vec()),
        (
            export,
            br#"{"messages":[{"type":"text","content":"private-text"}]}"#.to_vec(),
        ),
    ];
    for (path, bytes) in &files {
        fs::write(path, bytes).unwrap();
    }
    let status = success(run(root.path(), &["--json", "--exported-dir", "chosen"]));
    assert_eq!(status["key_files"].as_array().unwrap().len(), 1);
    assert!(status.get("progress").is_none());
    assert!(status.get("unreadable_transcriptions").is_none());
    assert_eq!(status["exports"]["files"], 1);
    assert_eq!(status["exports"]["bytes"], files[2].1.len());
    assert!(!status.to_string().contains("private-text"));
    for (path, bytes) in files {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    let runtimes: Vec<_> = fs::read_dir(root.path().join("runtime/accounts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(runtimes.len(), 1);
    assert_eq!(fs::read_dir(runtimes[0].join("cache")).unwrap().count(), 0);
    assert!(!root.path().join("runtime/bootstrap").exists());
    assert!(!account.join("synthetic-db").exists());
    assert!(!account.join("decrypted").exists());
}

#[test]
fn progress_rejects_unknown_options_without_python() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), &["--unknown"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--unknown"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
