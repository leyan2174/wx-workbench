//! Public operation CLI launched by cmd.exe against a synthetic account only.
#![cfg(windows)]
#[path = "../src/private_file.rs"]
#[allow(dead_code)] // Shared production module; this fixture does not exercise every entry point.
mod private_file;

#[path = "support/mcp_failure.rs"]
mod mcp_failure;
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;

use mcp_failure::safe_failure;
use serde_json::Value;
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Component, PathBuf, Prefix},
    process::Command,
};
use support::Account;

#[test]
fn cmd_drive_environment_allows_public_toolkit_status_operation() {
    let home = tempfile::tempdir().unwrap();
    let mut account = Account::new(home.path(), "CMD_OPERATION");
    let cwd = account.root().join("cmd working directory");
    fs::create_dir(&cwd).unwrap();
    let drive = match cwd.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) => char::from(drive),
            other => panic!("cmd drive regression requires a drive-letter temp path: {other:?}"),
        },
        other => panic!("cmd drive regression requires an absolute temp path: {other:?}"),
    };
    let config = account.root().join("config.json");
    account.start();

    let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
    // cmd parses /c in its own command line, including a mixed-separator /cmd.exe path.
    let mut command = Command::new(PathBuf::from(&system_root).join("System32").join("cmd.exe"));
    command
        .env_clear()
        .env("SystemRoot", &system_root)
        .env("WINDIR", &system_root)
        .env("PATH", "")
        .env("TEMP", home.path())
        .env("TMP", home.path())
        .env("WX_CLI_CONFIG", &config)
        .env("WX_CLI_HOME", home.path())
        .env("WX_CMD_TEST_CWD", &cwd)
        .env("WX_CMD_TEST_EXE", env!("CARGO_BIN_EXE_wx"))
        .current_dir(account.root())
        .creation_flags(0x08000000);
    for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        command.env(name, home.path());
    }

    // Delayed expansion observes =<drive>: after cd, not before the command is parsed.
    // Launch wx directly from this cmd so its native environment block is inherited.
    let script = format!(
        r#"cd /D "!WX_CMD_TEST_CWD!" && if "!={drive}:!"=="!WX_CMD_TEST_CWD!" ("!WX_CMD_TEST_EXE!" toolkit status --json) else (echo cmd drive pseudo-variable was not established 1>&2 & exit /b 91)"#
    );
    command.args(["/D", "/V:ON", "/C"]).raw_arg(script);
    println!("COMMAND: {command:?}");
    let output = command.output().expect("launch real cmd.exe and wx");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for (stream, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        assert!(
            !text.contains("Invalid operation environment"),
            "public operation rejected cmd environment ({stream}): {text}"
        );
    }
    assert!(
        output.status.success(),
        "cmd/wx failed: {}; stdout: {stdout}; stderr: {stderr}",
        output.status
    );
    let status: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid status JSON: {error}; stdout: {stdout}"));
    assert_eq!(status["implementation"], "native-rust");
    let commands = status["native_commands"]
        .as_array()
        .expect("toolkit status must enumerate native commands");
    for expected in ["status", "progress", "decrypt"] {
        assert!(commands.iter().any(|command| command == expected));
    }
    assert!(commands
        .iter()
        .all(|command| !command.as_str().unwrap().starts_with("run ")));
}
