//! Video recovery and publication security with synthetic inputs.
#[path = "support/bootstrap.rs"]
mod bootstrap;
#[cfg(feature = "sns-wasm-test-asset")]
#[path = "../src/adapters/wechat/media/sns_keystream.rs"]
#[allow(dead_code)]
mod video;
use bootstrap::BootstrapCleanup;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
const SECRET: &str = "SYNTHETIC_CREDENTIAL_MUST_NOT_LEAK";
#[cfg(feature = "sns-wasm-test-asset")]
fn repo() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if root.join("src/main.rs").exists() {
        root
    } else {
        root.join("../../..")
    }
}

fn run_wx(root: &Path, args: &[&str]) -> std::process::Output {
    let before = protected_snapshot(root);
    // 正常集成测试由 Cargo 指定二进制；独立 harness 必须显式指定已构建二进制。
    let exe = option_env!("CARGO_BIN_EXE_wx")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("WX_SECURITY_WX_EXE").map(PathBuf::from))
        .expect("独立 harness 的 CLI 测试需要 WX_SECURITY_WX_EXE");
    let output = std::process::Command::new(exe)
        .args(args)
        .current_dir(root)
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env("WX_CLI_HOME", root.join("runtime"))
        .env("WX_CLI_CONFIG", root.join("absent-config.json"))
        .env("PATH", "")
        .output()
        .unwrap();
    bootstrap::assert_only_bootstrap(&root.join("runtime"));
    assert!(
        before == protected_snapshot(root),
        "CLI changed protected fixture data"
    );
    output
}

fn protected_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path == root.join("runtime") {
                continue;
            }
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "unexpected fixture link");
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if kind.is_dir() {
                result.insert(relative, None);
                visit(root, &path, result);
            } else {
                result.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn process_failure(output: std::process::Output) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "意外成功: {text}");
    assert!(!text.contains(SECRET), "进程输出泄露合成凭据");
    text
}

#[test]
fn security_cli_video_failure_never_publishes_or_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let _bootstrap = BootstrapCleanup(dir.path().join("runtime"));
    let input = dir.path().join("input.bin");
    let key = dir.path().join("key.txt");
    let wasm = dir.path().join("invalid.wasm");
    let output = dir.path().join("new/output.mp4");
    fs::write(&input, [0; 64]).unwrap();
    fs::write(&wasm, b"INVALID_ASSET").unwrap();
    for value in [SECRET.to_owned(), "1".repeat(1025), "42".to_owned()] {
        fs::write(&key, &value).unwrap();
        process_failure(run_wx(
            dir.path(),
            &[
                "media",
                "video",
                "decode",
                input.to_str().unwrap(),
                output.to_str().unwrap(),
                "--key-file",
                key.to_str().unwrap(),
            ],
        ));
        assert!(!output.parent().unwrap().exists());
        assert_eq!(fs::read(&key).unwrap(), value.as_bytes());
    }
    let error = process_failure(run_wx(
        dir.path(),
        &[
            "media",
            "video",
            "decode",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--key-file",
            key.to_str().unwrap(),
            "--wasm",
            wasm.to_str().unwrap(),
        ],
    ));
    assert!(error.contains("WASM"));
    assert!(!output.parent().unwrap().exists());
    let occupied = dir.path().join("occupied.mp4");
    fs::write(&occupied, b"KEEP").unwrap();
    for target in [&input, &key, &occupied] {
        process_failure(run_wx(
            dir.path(),
            &[
                "media",
                "video",
                "decode",
                input.to_str().unwrap(),
                target.to_str().unwrap(),
                "--key-file",
                key.to_str().unwrap(),
            ],
        ));
    }
    assert_eq!(fs::read(&input).unwrap(), [0; 64]);
    assert_eq!(fs::read(&key).unwrap(), b"42");
    assert_eq!(fs::read(&occupied).unwrap(), b"KEEP");
    assert_eq!(fs::read(&wasm).unwrap(), b"INVALID_ASSET");
}

#[test]
#[cfg(feature = "sns-wasm-test-asset")]
fn security_video_rejects_tampered_assets_and_recovers_after_bad_key() {
    let dir = tempfile::tempdir().unwrap();
    let asset = repo().join("src/adapters/wechat/media/assets/wasm_video_decode.wasm");
    let original = fs::read(&asset).unwrap();
    let tampered = dir.path().join("tampered.wasm");
    let mut bytes = original.clone();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&tampered, &bytes).unwrap();
    assert!(matches!(
        video::SnsKeystream::new(&tampered, video::RuntimeLimits::default()),
        Err(video::KeystreamError::UnsupportedAsset)
    ));
    let huge = dir.path().join("oversized.wasm");
    fs::File::create(&huge)
        .unwrap()
        .set_len(4 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        video::SnsKeystream::new(&huge, video::RuntimeLimits::default()),
        Err(video::KeystreamError::UnsupportedAsset)
    ));
    let runtime = video::SnsKeystream::new(&asset, video::RuntimeLimits::default()).unwrap();
    let expected = runtime.keystream("42", 16).unwrap();
    for key in [SECRET.to_owned(), "1".repeat(1025)] {
        let error = runtime.keystream(&key, 16).unwrap_err();
        assert!(!format!("{error:?} {error}").contains(SECRET));
        assert_eq!(runtime.keystream("42", 16).unwrap(), expected);
    }
    let invalid = vec![0; 64];
    assert_eq!(
        runtime.restore_video("42", &invalid),
        Err(video::KeystreamError::InvalidMp4)
    );
    assert_eq!(invalid, vec![0; 64]);
    assert_eq!(fs::read(&asset).unwrap(), original);
}
