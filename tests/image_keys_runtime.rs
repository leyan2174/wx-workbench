//! Synthetic offline CLI -> daemon -> operation-worker tests; no real account access.
#![cfg(windows)]

#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "support/cli_output.rs"]
mod cli_output;
#[path = "support/key_store.rs"]
mod key_store;

use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use key_store::dpapi;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};
use zeroize::Zeroizing;

const ACCOUNT: &str = "wxid_fixture_cfcd";
const MAGIC: &[u8] = b"WXKEYS\0\x01";
const V2_MAGIC: &[u8] = &[0x07, 0x08, b'V', b'2', 0x08, 0x07];
const DECODED: &[u8] = &[0xff, 0xd8, 0xff, 0xff, 0xd9];

// Mirrors offline.rs::tests::synthetic: uin=0 is the first candidate, XOR=0.
// This is the decoder's minimal JPEG-signature fixture, not a displayable JPEG.
fn synthetic() -> ([u8; 16], Vec<u8>) {
    let reference = format!("{:x}", md5::compute(b"0wxid_fixture"));
    let key: [u8; 16] = reference.as_bytes()[..16].try_into().unwrap();
    let mut padded = [13; 16];
    padded[..3].copy_from_slice(&DECODED[..3]);
    let mut block = GenericArray::clone_from_slice(&padded);
    aes::Aes128::new((&key).into()).encrypt_block(&mut block);
    let mut bytes = V2_MAGIC.to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&block);
    bytes.extend_from_slice(&DECODED[3..]);
    (key, bytes)
}

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let f = Self(tempfile::tempdir().unwrap());
        fs::create_dir_all(f.path(&format!("{ACCOUNT}/db_storage"))).unwrap();
        fs::create_dir_all(f.thumbnail().parent().unwrap()).unwrap();
        fs::create_dir(f.path("samples")).unwrap();
        fs::write(f.thumbnail(), synthetic().1).unwrap();
        fs::create_dir(f.path("ambient")).unwrap();
        fs::write(
            f.path("ambient/config.json"),
            b"invalid ambient account config",
        )
        .unwrap();
        fs::write(
            f.path("config.json"),
            serde_json::to_vec_pretty(&json!({
                "db_dir": f.path(&format!("{ACCOUNT}/db_storage")),
                "keys_file": f.path("all_keys.json"),
                "key_store": f.path("keys.dpapi"),
                "decrypted_dir": f.path("decrypted"),
                "wechat_process": "wx-synthetic-offline-never-scan.exe",
                "fixture_marker": "configuration must remain byte-identical"
            }))
            .unwrap(),
        )
        .unwrap();
        f
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn thumbnail(&self) -> PathBuf {
        self.path(&format!(
            "{ACCOUNT}/msg/attach/contact/month/Img/fixture_t.dat"
        ))
    }

    fn record(&self) -> Value {
        let encrypted = fs::read(self.path("keys.dpapi")).unwrap();
        assert!(encrypted.starts_with(MAGIC));
        let plain = dpapi::transform(&encrypted[MAGIC.len()..], true).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }

    fn seed_materials(&self) -> Value {
        key_store::seed_unverified(
            &self.path("config.json"),
            &json!({"message/message_0.db": "41".repeat(32)}),
        );
        // The shared fixture seeds DB/image records; add only the missing account fixture here.
        let mut record = self.record();
        record["account_key"] = json!({"bytes": vec![0x52u8; 32], "verification": "unverified"});
        record["image_key"] = json!({"bytes": vec![0x63u8; 17], "verification": "unverified"});
        let plain = Zeroizing::new(serde_json::to_vec(&record).unwrap());
        let protected = dpapi::transform(&plain, false).unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&protected);
        fs::write(self.path("keys.dpapi"), bytes).unwrap();
        record
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = vec![
            "toolkit",
            "find-image-key",
            "--offline",
            "--timeout",
            "10",
            "--max-mib",
            "1",
        ];
        command.extend_from_slice(args);
        self.run_command(&command)
    }

    fn run_command(&self, args: &[&str]) -> Output {
        let config = self.path("config.json");
        let original = fs::read(&config).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(args)
            .current_dir(self.path("ambient"))
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_DAEMON_OPERATION_WORKER")
            .env_remove("WX_DAEMON_TASK_WORKER")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_CONTACTS")
            .env_remove("WECHAT_EXPORT_USERS")
            .env("WX_CLI_CONFIG", &config)
            .env("WX_CLI_HOME", self.path("runtime"))
            .env("PATH", "");
        let output = cli_output::output(&mut command, self.0.path(), Duration::from_secs(60));
        assert_eq!(fs::read(config).unwrap(), original);
        assert_eq!(
            fs::read(self.path("ambient/config.json")).unwrap(),
            b"invalid ambient account config"
        );
        assert!(!self.path("all_keys.json").exists());
        assert!(!self.path("decrypted").exists());
        assert_eq!(
            fs::read_dir(self.path(&format!("{ACCOUNT}/db_storage")))
                .unwrap()
                .count(),
            0
        );

        // The public command must have used the authenticated account daemon, not a fallback CLI.
        let accounts: Vec<_> = fs::read_dir(self.path("runtime/accounts"))
            .unwrap_or_else(|error| {
                panic!(
                    "daemon runtime missing: {error}; status={} stdout={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                )
            })
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(accounts.len(), 1);
        let daemon: Value =
            serde_json::from_slice(&fs::read(accounts[0].join("daemon.pid")).unwrap()).unwrap();
        assert!(bootstrap::verified_process(&daemon).unwrap().is_some());
        let key = String::from_utf8(synthetic().0.to_vec()).unwrap();
        for stream in [&output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(stream).contains(&key),
                "CLI leaked synthetic AES material"
            );
        }
        output
    }

    fn success(&self, args: &[&str], saved: bool) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["engine"], "rust");
        assert_eq!(report["inference_mode"], "offline");
        assert_eq!(report["xor_policy"], "thumbnail_tail_vote");
        assert_eq!(report["aes_template_verified"], true);
        assert_eq!(report["config_updated"], false);
        assert_eq!(report["key_store_updated"], saved);
        assert_eq!(report["keys_redacted"], true);
        assert_eq!(fs::read(self.thumbnail()).unwrap(), synthetic().1);
        report
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(bootstrap::RuntimeCleanup(self.path("runtime")));
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn assert_image(record: &Value) {
    let mut expected = synthetic().0.to_vec();
    expected.push(0);
    assert_eq!(record["version"], 1);
    assert_eq!(
        record["image_key"],
        json!({"bytes": expected, "verification": "verified"})
    );
}

#[test]
fn monitor_reuses_daemon_image_snapshot_without_rewriting_material() {
    let f = Fixture::new();
    f.success(&[], true);
    let before = fs::read(f.path("keys.dpapi")).unwrap();
    let sample = f.path("samples/reused.jpg");
    let output = f.run_command(&[
        "toolkit",
        "find-image-key-monitor",
        "--authorize-memory-scan",
        "--no-save",
        "--timeout",
        "10",
        "--scan-seconds",
        "1",
        "--max-mib",
        "1",
        "--sample-input",
        arg(&f.thumbnail()),
        "--sample-output",
        arg(&sample),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["existing_key_valid"], true);
    assert_eq!(report["key_store_updated"], false);
    assert_eq!(report["rounds"], 1);
    assert_eq!(fs::read(sample).unwrap(), DECODED);
    assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), before);

    // External replacement is visible only after explicit invalidation/restart.
    // A worker that independently decrypts the store would fail this second run.
    let damaged = b"synthetic unreadable replacement";
    fs::write(f.path("keys.dpapi"), damaged).unwrap();
    let output = f.run_command(&[
        "toolkit",
        "find-image-key-monitor",
        "--authorize-memory-scan",
        "--no-save",
        "--timeout",
        "10",
        "--scan-seconds",
        "1",
        "--max-mib",
        "1",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["existing_key_valid"], true);
    assert_eq!(report["key_store_updated"], false);
    assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), damaged);
}

#[test]
fn offline_save_preserves_database_and_account_material_and_publishes_sample() {
    let f = Fixture::new();
    let before = f.seed_materials();
    let sample = f.path("samples/decoded.jpg");
    let report = f.success(
        &[
            "--sample-input",
            arg(&f.thumbnail()),
            "--sample-output",
            arg(&sample),
        ],
        true,
    );
    let after = f.record();
    assert_image(&after);
    assert_eq!(after["account"], before["account"]);
    assert_eq!(after["account_key"], before["account_key"]);
    assert_eq!(after["database_keys"], before["database_keys"]);
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(fs::read(&sample).unwrap(), DECODED);
    assert_eq!(
        report["sample"],
        json!({"path": sample, "format": "jpg", "bytes": DECODED.len()})
    );
}

#[test]
fn offline_save_creates_current_dpapi_store_without_rewriting_configuration() {
    let f = Fixture::new();
    assert!(!f.path("keys.dpapi").exists());
    let report = f.success(&[], true);
    let record = f.record();
    assert_image(&record);
    assert_eq!(record["revision"], 1);
    assert!(record["account_key"].is_null());
    assert_eq!(record["database_keys"], json!({}));
    assert!(report["sample"].is_null());
}

#[test]
fn offline_no_save_preserves_existing_dpapi_bytes_but_can_publish_sample() {
    let f = Fixture::new();
    f.seed_materials();
    let before = fs::read(f.path("keys.dpapi")).unwrap();
    let sample = f.path("samples/no-save.jpg");
    let report = f.success(
        &[
            "--no-save",
            "--sample-input",
            arg(&f.thumbnail()),
            "--sample-output",
            arg(&sample),
        ],
        false,
    );
    assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), before);
    assert_eq!(fs::read(&sample).unwrap(), DECODED);
    assert_eq!(report["sample"]["bytes"], DECODED.len());
}

#[test]
fn offline_no_save_does_not_create_a_key_store() {
    let f = Fixture::new();
    let report = f.success(&["--no-save"], false);
    assert!(!f.path("keys.dpapi").exists());
    assert!(report["sample"].is_null());
}

#[test]
fn offline_no_save_bypasses_legacy_and_damaged_stores_that_save_refuses() {
    let legacy = br#"{"image_aes_key":"synthetic-old-image-secret","image_xor_key":136}"#.to_vec();
    let mut damaged = MAGIC.to_vec();
    damaged.extend_from_slice(b"not-a-dpapi-blob");
    for bytes in [legacy, damaged] {
        let f = Fixture::new();
        fs::write(f.path("keys.dpapi"), &bytes).unwrap();
        let config = fs::read(f.path("config.json")).unwrap();
        let refused = f.run(&[]);
        assert!(
            !refused.status.success(),
            "save unexpectedly accepted an unusable store"
        );
        assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), bytes);

        let report = f.success(&["--no-save"], false);
        assert!(report["sample"].is_null());
        assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), bytes);
        assert_eq!(fs::read(f.path("config.json")).unwrap(), config);
        assert_eq!(fs::read_dir(f.path("samples")).unwrap().count(), 0);
    }
}

#[test]
fn offline_missing_thumbnail_fails_without_creating_or_changing_material() {
    for seeded in [false, true] {
        let f = Fixture::new();
        if seeded {
            f.seed_materials();
        }
        let before = fs::read(f.path("keys.dpapi")).ok();
        // A valid DAT outside the legacy thumbnail layout must not be considered.
        fs::rename(
            f.thumbnail(),
            f.path(&format!("{ACCOUNT}/msg/attach/outside_t.dat")),
        )
        .unwrap();
        let output = f.run(&[]);
        assert!(
            !output.status.success(),
            "missing thumbnail unexpectedly succeeded"
        );
        assert_eq!(fs::read(f.path("keys.dpapi")).ok(), before);
        assert_eq!(fs::read_dir(f.path("samples")).unwrap().count(), 0);
    }
}

#[test]
fn offline_sample_decode_failure_does_not_commit_material_or_publish_output() {
    let f = Fixture::new();
    f.seed_materials();
    let before = fs::read(f.path("keys.dpapi")).unwrap();
    let input = f.thumbnail().with_file_name("broken.dat");
    // Accepted by sample preparation, rejected by the real decoder after offline inference.
    fs::write(&input, V2_MAGIC).unwrap();
    let output_path = f.path("samples/broken.jpg");
    let output = f.run(&[
        "--sample-input",
        arg(&input),
        "--sample-output",
        arg(&output_path),
    ]);
    assert!(
        !output.status.success(),
        "invalid sample unexpectedly succeeded"
    );
    assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), before);
    assert!(!output_path.exists());
    assert_eq!(fs::read_dir(f.path("samples")).unwrap().count(), 0);
    assert_eq!(fs::read(f.thumbnail()).unwrap(), synthetic().1);
    assert_eq!(fs::read(input).unwrap(), V2_MAGIC);
}

#[test]
fn offline_rejects_plaintext_at_store_path_before_reading_samples_without_leaking() {
    use std::os::windows::fs::OpenOptionsExt;

    let f = Fixture::new();
    let secret = "synthetic-legacy-secret-must-never-reach-cli-output";
    let legacy = serde_json::to_vec(&json!({
        "image_aes_key": secret,
        "image_xor_key": 136,
        "message/message_0.db": {"enc_key": secret}
    }))
    .unwrap();
    fs::write(f.path("keys.dpapi"), &legacy).unwrap();
    let config = fs::read(f.path("config.json")).unwrap();

    // Any attempt to open this thumbnail for inference would fail with a sharing violation.
    // The existing-store refusal must take precedence over offline sample access.
    let pinned = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(f.thumbnail())
        .unwrap();
    let output = f.run(&[]);
    drop(pinned);
    assert!(
        !output.status.success(),
        "legacy plaintext unexpectedly accepted"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !diagnostic.contains(secret),
        "CLI leaked synthetic legacy material"
    );
    assert!(
        diagnostic.contains("Legacy key material is unsupported"),
        "missing typed legacy-format refusal"
    );
    assert!(
        diagnostic.contains("explicitly initialize"),
        "missing explicit initialization guidance"
    );
    assert!(!diagnostic.contains("aes_template_verified"));
    assert_eq!(fs::read(f.path("keys.dpapi")).unwrap(), legacy);
    assert_eq!(fs::read(f.path("config.json")).unwrap(), config);
    assert_eq!(fs::read(f.thumbnail()).unwrap(), synthetic().1);
    assert_eq!(fs::read_dir(f.path("samples")).unwrap().count(), 0);
}
