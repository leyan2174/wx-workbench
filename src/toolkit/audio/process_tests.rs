use super::*;
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

pub(super) fn fake_ffmpeg() -> &'static Path {
    static HELPER: OnceLock<tempfile::TempDir> = OnceLock::new();
    HELPER
        .get_or_init(|| {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("helper.rs");
            fs::write(&source, include_str!("process_fixture.rs")).unwrap();
            let mut command = Command::new("rustc");
            command
                .arg(&source)
                .arg("-o")
                .arg(root.path().join("helper.exe"));
            let result = crate::windows_process::managed::output(
                &mut command,
                Instant::now() + Duration::from_secs(60),
                1024 * 1024,
                || false,
            )
            .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            root
        })
        .path()
}

fn run_failure(mode: &str, cancel: bool) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.silk");
    fs::copy(tests::fixtures().join("tone.silk"), &source).unwrap();
    let output = temp.path().join("valid.mp3");
    fs::write(&output, b"old valid output").unwrap();
    let executable = temp.path().join(format!("{mode}.exe"));
    fs::copy(fake_ffmpeg().join("helper.exe"), &executable).unwrap();
    let start = Instant::now();
    let result = convert_controlled(
        &source,
        &output,
        &executable,
        start + Duration::from_secs(1),
        || cancel && start.elapsed() > Duration::from_millis(200),
    );
    assert!(result.is_err());
    let error = format!("{:#}", result.unwrap_err());
    let expected = if cancel {
        "cancelled"
    } else if mode == "flood" {
        "output limit"
    } else {
        "deadline"
    };
    assert!(error.contains(expected), "{error}");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(fs::read(&output).unwrap(), b"old valid output");
    assert_eq!(
        fs::read_dir(temp.path()).unwrap().count(),
        3,
        "temporary artifacts leaked"
    );
}

#[test]
fn direct_ffmpeg_hang_preserves_old_output() {
    run_failure("hang", false);
}

#[test]
fn ffmpeg_failure_does_not_expose_process_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.silk");
    fs::copy(tests::fixtures().join("tone.silk"), &source).unwrap();
    let output = temp.path().join("valid.mp3");
    fs::write(&output, b"old valid output").unwrap();
    let executable = temp.path().join("failure.exe");
    fs::copy(fake_ffmpeg().join("helper.exe"), &executable).unwrap();
    let error = convert_silk_to_mp3_with_ffmpeg(&source, &output, &executable).unwrap_err();
    assert!(error.to_string().contains("ffmpeg failed"));
    assert!(!format!("{error:#} {error:?}").contains("SYNTHETIC_PRIVATE_KEY"));
    assert_eq!(fs::read(&output).unwrap(), b"old valid output");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}

#[test]
fn direct_ffmpeg_output_flood_preserves_old_output() {
    run_failure("flood", false);
}

#[test]
fn direct_ffmpeg_cancellation_preserves_old_output() {
    run_failure("hang", true);
}

#[test]
fn publication_failure_preserves_old_output() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.silk");
    fs::copy(tests::fixtures().join("tone.silk"), &source).unwrap();
    let output = temp.path().join("valid.mp3");
    fs::write(&output, b"old valid output").unwrap();
    let executable = temp.path().join("success.exe");
    fs::copy(fake_ffmpeg().join("helper.exe"), &executable).unwrap();
    // Share reads/writes but forbid replacement/deletion, including TempPath::persist.
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&output)
        .unwrap();
    let error = convert_silk_to_mp3_with_ffmpeg(&source, &output, &executable).unwrap_err();
    assert!(error.to_string().contains("publish MP3"), "{error:#}");
    assert_eq!(fs::read(&output).unwrap(), b"old valid output");
    drop(lock);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}
