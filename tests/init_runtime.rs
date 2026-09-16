//! Initialization recovery with synthetic persisted material; no acquisition is needed.
#![cfg(windows)]

#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "support/cli_output.rs"]
mod cli_output;
#[path = "support/key_store.rs"]
mod key_store;

use serde_json::{json, Value};
use std::{
    fs,
    os::windows::fs::OpenOptionsExt,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

fn initialize(root: &Path) -> Output {
    initialize_with(root, &[])
}

fn initialize_with(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .arg("init")
        .args(args)
        .current_dir(root)
        .env("WX_CLI_CONFIG", root.join("config.json"))
        .env("WX_CLI_HOME", root.join("home"))
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_DAEMON_OPERATION_WORKER")
        .env_remove("WX_DAEMON_TASK_WORKER")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("PATH", "");
    cli_output::output(&mut command, root, Duration::from_secs(30))
}

#[test]
fn existing_keys_do_not_mask_configuration_failure_and_retry_finishes_initialization() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("home"));
    let database = root.path().join("wxid_synthetic/db_storage");
    fs::create_dir_all(&database).unwrap();
    let config = root.path().join("config.json");
    let mut settings = json!({
        "db_dir": database,
        "keys_file": "all_keys.json",
        "key_store": "keys.dpapi",
        "decrypted_dir": "decrypted",
        "wechat_process": "wx-synthetic-init-no-scan.exe",
        "unrelated": {"preserve": true}
    });
    fs::write(&config, serde_json::to_vec(&settings).unwrap()).unwrap();
    key_store::seed(&config, &json!({"contact/contact.db": "41".repeat(32)}));
    let store = root.path().join("keys.dpapi");
    let ciphertext = fs::read(&store).unwrap();
    settings.as_object_mut().unwrap().remove("key_store");
    let incomplete = serde_json::to_vec(&settings).unwrap();
    fs::write(&config, &incomplete).unwrap();

    // Permit reading the selected config but prevent its atomic replacement.
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&config)
        .unwrap();
    let failed = initialize_with(root.path(), &["--key-provider", "memory"]);
    assert!(
        !failed.status.success(),
        "existing keys must not bypass configuration publication"
    );
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("配置提交失败"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(fs::read(&config).unwrap(), incomplete);
    assert_eq!(fs::read(&store).unwrap(), ciphertext);
    drop(lock);

    let completed = initialize_with(root.path(), &["--key-provider", "memory"]);
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    let saved: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(saved["key_store"], "keys.dpapi");
    let mut expected = settings;
    expected["key_store"] = json!("keys.dpapi");
    assert_eq!(saved, expected);
    assert_eq!(fs::read(&store).unwrap(), ciphertext);
    assert!(!root.path().join("all_keys.json").exists());
    assert!(!root.path().join("decrypted").exists());
    for output in [&failed, &completed] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains("扫描加密密钥"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&"41".repeat(32)));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(&"41".repeat(32)));
    }
}

#[test]
fn default_saved_provider_never_falls_back_to_memory_scanning() {
    let root = tempfile::tempdir().unwrap();
    let _cleanup = bootstrap::RuntimeCleanup(root.path().join("home"));
    let database = root.path().join("wxid_synthetic/db_storage/contact");
    fs::create_dir_all(&database).unwrap();
    let source = database.join("contact.db");
    let source_bytes = [0x31; 16];
    fs::write(&source, source_bytes).unwrap();
    let config = root.path().join("config.json");
    let settings = json!({
        "db_dir": database.parent().unwrap(),
        "keys_file": "all_keys.json",
        "key_store": "keys.dpapi",
        "decrypted_dir": "decrypted",
        "wechat_process": "wx-synthetic-init-must-not-run.exe"
    });
    let config_bytes = serde_json::to_vec(&settings).unwrap();
    fs::write(&config, &config_bytes).unwrap();

    let output = initialize(root.path());
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{diagnostic}");
    assert!(diagnostic.contains("没有已保存的账号密钥"), "{diagnostic}");
    assert!(!diagnostic.contains("wx-synthetic-init-must-not-run.exe"));
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(fs::read(&config).unwrap(), config_bytes);
    assert!(!root.path().join("keys.dpapi").exists());
    assert!(!root.path().join("all_keys.json").exists());
    assert!(!root.path().join("decrypted").exists());
}
