//! Current-checkout CLI contracts, using only synthetic temporary account material.
#![cfg(windows)]

#[path = "support/key_store.rs"]
mod key_store;
#[path = "support/managed_process.rs"]
mod windows_process;

use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

// Retain directories as well as bytes: an empty runtime/cache directory is a side effect.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "fixture must not acquire symlinks");
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if kind.is_dir() {
                files.insert(relative, None);
                visit(root, &path, files);
            } else {
                assert!(kind.is_file());
                files.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        for name in ["home", "temp", "appdata", "localappdata"] {
            fs::create_dir(fixture.path(name)).unwrap();
        }
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        let executable = Path::new(env!("CARGO_BIN_EXE_wx")).canonicalize().unwrap();
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        // Match scripts/check-quality.ps1 ownership policy, using the actual binary
        // ancestry rather than a mutable CARGO_TARGET_DIR environment override.
        let owner_file = executable
            .parent()
            .unwrap()
            .ancestors()
            .find_map(|directory| {
                let marker = directory.join(".checkout-owner");
                marker
                    .try_exists()
                    .expect("must be able to inspect build directory ownership")
                    .then_some(marker)
            });
        if let Some(owner_file) = owner_file {
            let owner = fs::read_to_string(owner_file).expect("read build directory owner");
            let owner = Path::new(owner.trim());
            assert!(
                owner.is_absolute(),
                "build directory owner must be an absolute checkout path"
            );
            assert_eq!(
                owner.canonicalize().expect("resolve build directory owner"),
                checkout,
                "build directory belongs to another checkout"
            );
        } else {
            assert!(
                executable.starts_with(&checkout),
                "external build directory must have a matching .checkout-owner"
            );
        }
        let test_executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(
            executable.parent(),
            test_executable.parent().unwrap().parent(),
            "wx and this integration test must come from the same build profile"
        );
        assert_eq!(executable.file_name().unwrap(), "wx.exe");

        let before = snapshot(self.0.path());
        let mut command = Command::new(executable);
        command.args(args).current_dir(self.0.path()).env_clear();
        // Windows loader variables only; never inherit account, worker, proxy or tool settings.
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (name, relative) in [
            ("WX_CLI_CONFIG", "config.json"),
            ("WX_CLI_HOME", "runtime"),
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
            ("TEMP", "temp"),
            ("TMP", "temp"),
        ] {
            command.env(name, self.path(relative));
        }
        command.env("PATH", "").env("NO_COLOR", "1");
        let output = windows_process::managed::output(
            &mut command,
            Instant::now() + Duration::from_secs(20),
            256 * 1024,
            || false,
        )
        .expect("current wx must finish under process-tree, time and output supervision");
        // Do not print snapshots: the configuration intentionally contains synthetic secrets.
        assert!(
            before == snapshot(self.0.path()),
            "CLI created, removed or rewrote fixture files/directories"
        );
        assert!(
            !self.path("runtime").exists(),
            "CLI created daemon runtime state"
        );
        output
    }
}

fn cli_refusal(output: Output, command: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a Clap usage refusal"
    );
    assert!(output.stdout.is_empty(), "rejected command wrote stdout");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("unrecognized subcommand"),
        "not a CLI parsing refusal"
    );
    assert!(
        stderr.contains(command),
        "refusal did not identify the obsolete command"
    );
    assert!(stderr.contains("Usage:"), "missing CLI usage diagnostic");
}

const BUSINESS_COMMANDS: &[(&str, &[&str])] = &[
    ("setup", &["setup"]),
    ("cleanup", &["cleanup"]),
    ("status", &["status"]),
    ("progress", &["progress"]),
    ("monitor", &["monitor"]),
    ("latency", &["latency"]),
    ("web", &["web"]),
    ("gui", &["gui"]),
    ("decrypt", &["database", "decrypt"]),
    (
        "transcribe-database-native",
        &["audio", "transcribe-message"],
    ),
    ("export-delta-native", &["chats", "export-delta"]),
    ("chat-plan-native", &["chats", "plan"]),
    ("transcribe-audio-native", &["audio", "transcribe"]),
    ("transcribe-chat-native", &["chats", "transcribe-manifest"]),
    ("decode-sns-video", &["media", "video", "decode"]),
    ("export-sns-native", &["moments", "export-snapshot"]),
    ("export-chats-native", &["chats", "export"]),
    ("export-emoticons", &["emoticons", "export"]),
    ("export-all", &["chats", "export-all"]),
    ("export-sns", &["moments", "export"]),
    ("export-messages", &["chats", "export-messages"]),
    ("decrypt-sns", &["moments", "archive"]),
    ("find-image-key", &["keys", "image"]),
    ("find-database-keys", &["keys", "database"]),
    ("find-image-key-monitor", &["keys", "watch-image"]),
    ("decode-images", &["media", "image", "decode-cache"]),
    ("decode-image", &["media", "image", "decode"]),
    (
        "batch-decrypt-images",
        &["media", "image", "decode-directory"],
    ),
    ("voice-batch", &["audio", "export"]),
    ("voice-to-mp3", &["audio", "convert"]),
    ("transcribe-chat", &["chats", "transcribe"]),
];

#[test]
fn business_command_help_parses_without_account_or_daemon_side_effects() {
    for &(_, command) in BUSINESS_COMMANDS {
        let fixture = Fixture::new();
        let mut args = command.to_vec();
        args.push("--help");
        let output = fixture.run(&args);
        assert!(output.status.success(), "help failed for {command:?}");
        assert!(
            output.stderr.is_empty(),
            "help wrote stderr for {command:?}"
        );
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            help.contains(&format!("Usage: wx.exe {}", command.join(" "))),
            "help did not resolve the full business command: {command:?}"
        );
    }
}

#[test]
fn removed_toolkit_entry_is_rejected_without_account_or_daemon_side_effects() {
    let fixture = Fixture::new();
    cli_refusal(fixture.run(&["toolkit"]), "toolkit");
    cli_refusal(fixture.run(&["toolkit", "--help"]), "toolkit");
    for &(legacy, _) in BUSINESS_COMMANDS {
        for args in [vec!["toolkit", legacy], vec!["toolkit", legacy, "--help"]] {
            cli_refusal(fixture.run(&args), "toolkit");
        }
    }
}

#[test]
fn unsupported_migrate_keys_is_rejected_without_creating_account_or_daemon_state() {
    // Unsupported commands must also reject these flags without creating state.
    for args in [
        vec!["migrate-keys"],
        vec!["migrate-keys", "--allow-unverified", "--cleanup-legacy"],
    ] {
        let fixture = Fixture::new();
        cli_refusal(fixture.run(&args), "migrate-keys");
        assert!(!fixture.path("config.json").exists());
    }
}

#[test]
fn removed_launcher_script_entries_are_cli_errors_without_side_effects() {
    // Script-style command spellings must be rejected by the CLI.
    for script in [
        "main.py",
        "monitor_web.py",
        "app_gui.py",
        "export_all_chats.py",
        "decrypt_db.py",
        "export_messages.py",
        "find_image_key.py",
        "find_image_key_monitor.py",
        "decrypt_sns.py",
        "export_sns.py",
        "voice_to_mp3.py",
        "batch_decrypt_images.py",
        "monitor.py",
        "latency_test.py",
        "setup.py",
        "cleanup.py",
    ] {
        let fixture = Fixture::new();
        let args = if script == "main.py" {
            vec![script, "status"]
        } else {
            vec![script]
        };
        cli_refusal(fixture.run(&args), script);
        assert!(!fixture.path("config.json").exists());
    }
}

#[test]
fn help_keeps_the_current_business_cli_without_migration_or_launcher_entries() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--help"]);
    assert!(output.status.success(), "current wx --help failed");
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Usage: wx"));
    let names: Vec<_> = help
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    for command in [
        "init",
        "mcp",
        "sessions",
        "history",
        "search",
        "contacts",
        "voices",
        "daemon",
        "setup",
        "cleanup",
        "status",
        "progress",
        "monitor",
        "latency",
        "web",
        "gui",
        "database",
        "audio",
        "chats",
        "media",
        "moments",
        "emoticons",
        "keys",
    ] {
        assert!(
            names.contains(&command),
            "missing formal command: {command}"
        );
    }
    for obsolete in ["toolkit", "migrate-keys", "wx-toolbox", ".py"] {
        assert!(
            !help.contains(obsolete),
            "obsolete entry remains in help: {obsolete}"
        );
    }
    assert!(!fixture.path("config.json").exists());
}

#[test]
fn shared_config_loader_refuses_legacy_image_fields_even_with_current_key_store() {
    const AES_SECRET: &str = "0123456789abcdef0123456789abcdef";
    const XOR_SECRET: &str = "synthetic-xor-secret-do-not-disclose";
    for legacy in [
        json!({"image_aes_key": AES_SECRET}),
        json!({"image_xor_key": XOR_SECRET}),
        json!({"image_aes_key": AES_SECRET, "image_xor_key": 173}),
        json!({"image_aes_key": null}),
        json!({"image_xor_key": null}),
    ] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("db_storage")).unwrap();
        let config_path = fixture.path("config.json");
        fs::write(
            &config_path,
            serde_json::to_vec(&json!({
                "db_dir": fixture.path("db_storage"),
                "keys_file": fixture.path("all_keys.json"),
                "key_store": fixture.path("keys.dpapi"),
                "decrypted_dir": fixture.path("decrypted"),
                "wechat_process": "synthetic-contract-no-wechat.exe"
            }))
            .unwrap(),
        )
        .unwrap();
        key_store::seed(&config_path, &json!({}));
        assert!(fs::read(fixture.path("keys.dpapi"))
            .unwrap()
            .starts_with(b"WXKEYS\0\x01"));
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config
            .as_object_mut()
            .unwrap()
            .extend(legacy.as_object().unwrap().clone());
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

        // daemon logs -> RuntimeContext::load -> load_config_at, before any IPC or scan.
        // daemon status swallows load errors; service operations may bootstrap on load failure.
        let output = fixture.run(&["daemon", "logs"]);
        for bytes in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(bytes);
            for secret in [AES_SECRET, XOR_SECRET, "173"] {
                assert!(
                    !text.contains(secret),
                    "configuration secret leaked into CLI output"
                );
            }
        }
        assert_eq!(
            output.status.code(),
            Some(1),
            "expected shared configuration refusal"
        );
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("Legacy inline image keys are unsupported"));
        assert!(stderr.contains("key_store"));
        assert!(stderr.contains("No files were changed"));
    }
}
