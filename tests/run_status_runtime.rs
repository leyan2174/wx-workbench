use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wx"))
        .args(["toolkit", "run"])
        .args(args)
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_CONFIG", root.join("account/config.json"))
        .env("WX_CLI_HOME", root.join("runtime"))
        .env("WX_WECHAT_DECRYPT_PYTHON", root.join("missing-python.exe"))
        .env("PATH", "")
        .current_dir(root)
        .output()
        .unwrap()
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
fn run_status_missing_config_and_short_alias_need_no_runtime() {
    let root = tempfile::tempdir().unwrap();
    for name in ["status", "-s"] {
        let status = success(run(root.path(), &[name, "--", "--json"]));
        assert_eq!(status["config_exists"], false);
        assert_eq!(status["databases"]["files"], 0);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn run_status_override_counts_without_changing_sources() {
    let root = tempfile::tempdir().unwrap();
    let account = root.path().join("account");
    fs::create_dir(&account).unwrap();
    let config = account.join("config.json");
    let keys = account.join("all_keys.json");
    let exported = root.path().join("chosen");
    fs::create_dir(&exported).unwrap();
    let transcript = exported.join("chat_transcribed.json");
    let files = [
        (config, br#"{"keys_file":"all_keys.json"}"#.to_vec()),
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
    assert!(!root.path().join("runtime").exists());
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
