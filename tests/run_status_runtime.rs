use serde_json::{json, Value};
#[path = "support/bootstrap.rs"]
mod bootstrap;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    command(root)
        .args(["toolkit", "run"])
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
        .env("WX_WECHAT_DECRYPT_PYTHON", root.join("missing-python.exe"))
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
fn run_status_missing_config_and_short_alias_use_bootstrap_only() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::BootstrapCleanup(root.path().join("runtime"));
    for name in ["status", "-s"] {
        let status = success(run(root.path(), &[name, "--", "--json"]));
        assert_eq!(status["config_exists"], false);
        assert_eq!(status["databases"]["files"], 0);
        assert!(!root.path().join("account").exists());
        assert!(root.path().join("runtime/bootstrap").is_dir());
        bootstrap::assert_only_bootstrap(&root.path().join("runtime"));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn run_status_override_counts_without_changing_sources() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("runtime"));
    let account = root.path().join("account");
    fs::create_dir(&account).unwrap();
    let config = account.join("config.json");
    let keys = account.join("all_keys.json");
    let exported = root.path().join("chosen");
    fs::create_dir(&exported).unwrap();
    let transcript = exported.join("chat_transcribed.json");
    let files = [
        (
            config,
            br#"{"db_dir":"synthetic-db","keys_file":"all_keys.json","decrypted_dir":"decrypted"}"#
                .to_vec(),
        ),
        (keys, b"invalid-json-key-must-not-be-read".to_vec()),
        (
            transcript,
            serde_json::to_vec(
                &json!({"messages":[{"type":"voice","transcription":"private-text"}]}),
            )
            .unwrap(),
        ),
    ];
    for (path, bytes) in &files {
        fs::write(path, bytes).unwrap();
    }
    let status = success(run(
        root.path(),
        &["status", "--", "--json", "--exported-dir", "chosen"],
    ));
    assert_eq!(status["key_files"].as_array().unwrap().len(), 1);
    assert_eq!(status["progress"], json!({"voices":1,"transcribed":1}));
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
fn run_status_rejects_unknown_options_without_python() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), &["status", "--", "--unknown"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--unknown"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
