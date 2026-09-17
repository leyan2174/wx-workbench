//! Unsupported key commands must fail in clap before configuration or daemon work.
#![cfg(windows)]

use std::{fs, os::windows::process::CommandExt, process::Command};

#[test]
fn unsupported_key_commands_cannot_load_config_or_modify_existing_material() {
    for args in [
        vec!["migrate-keys"],
        vec!["migrate-keys", "--allow-unverified", "--cleanup-legacy"],
        vec!["key", "migrate"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let config = root.path().join("config.json");
        fs::create_dir(&home).unwrap();
        for name in ["config.json", "all_keys.json", "account_key.dpapi"] {
            fs::write(
                root.path().join(name),
                b"invalid synthetic material; must stay untouched",
            )
            .unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .env_clear()
            .current_dir(root.path())
            .creation_flags(0x08000000);
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "WX_CLI_HOME",
        ] {
            command.env(name, &home);
        }
        command
            .env("PATH", "")
            .env("TEMP", root.path())
            .env("TMP", root.path())
            .env("WX_CLI_CONFIG", &config)
            .args(&args);
        println!("COMMAND: {command:?}");
        let output = command.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(
            stderr.contains(&format!("unrecognized subcommand '{}'", args[0])),
            "{stderr}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 4);
        for name in ["config.json", "all_keys.json", "account_key.dpapi"] {
            assert_eq!(
                fs::read(root.path().join(name)).unwrap(),
                b"invalid synthetic material; must stay untouched"
            );
        }
    }
}
