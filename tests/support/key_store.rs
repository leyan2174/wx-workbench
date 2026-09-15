//! Seed production encrypted storage through the real explicit migration operation.
use std::{path::Path, process::Command};

// Most runtime fixtures verify source keys; decrypt/timeline fixtures explicitly
// use migrate_with_unverified instead. Keep both entry points in shared support.
#[allow(dead_code)]
pub fn migrate(binary: &Path, config: &Path, home: &Path) {
    migrate_with_unverified(binary, config, home, false);
}

pub fn migrate_with_unverified(binary: &Path, config: &Path, home: &Path, allow_unverified: bool) {
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        config.canonicalize().unwrap().starts_with(temp),
        "key fixture must use an isolated temporary configuration"
    );
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    let run = |args: &[&str]| {
        Command::new(binary)
            .args(args)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env("WX_CLI_CONFIG", config)
            .env("WX_CLI_HOME", home)
            .current_dir(config.parent().unwrap())
            .output()
            .unwrap()
    };
    let mut args = vec!["migrate-keys", "--cleanup-legacy"];
    if allow_unverified {
        args.push("--allow-unverified");
    }
    let result = run(&args);
    assert!(
        result.status.success(),
        "synthetic key migration failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    if before.get("key_store").is_none_or(|value| value.is_null()) {
        let result = run(&["daemon", "stop"]);
        assert!(
            result.status.success(),
            "synthetic migration daemon stop failed"
        );
    }
}
