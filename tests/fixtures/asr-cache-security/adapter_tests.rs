use super::{cached::*, local, openai, Backend};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};
const SILK: &[u8] = include_bytes!("../audio/silence.silk");

fn request(path: &Path) -> CachedRequest<'_> {
    CachedRequest {
        cache_path: path,
        account: "synthetic-a",
        username: "peer",
        source: "message/media_0.db",
        local_id: 1,
        create_time: 10,
        silk: SILK,
    }
}

fn program() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let exe = dir.path().join("probe.exe");
            let mut command = Command::new("rustc");
            command
                .args(["--edition=2021", "--crate-name", "cache_security_probe"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("probe.rs"))
                .arg("-o")
                .arg(&exe);
            println!("COMMAND: {command:?}");
            let out = command.output().unwrap();
            println!(
                "STDOUT: {}\nSTDERR: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.status.success());
            (dir, exe)
        })
        .1
}

#[test]
fn denied_cloud_checks_authorization_before_invalid_audio_account_or_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    fs::write(&path, b"broken-secret-marker").unwrap();
    // 只构造客户端；无服务监听、无上传。若授权顺序退化，精确错误断言失败。
    let client = openai::OpenAiTranscriber::new(openai::OpenAiConfig {
        base_url: "http://127.0.0.1:9/v1".into(),
        model: "synthetic".into(),
        language: None,
        api_key: "SYNTHETIC_CREDENTIAL".into(),
        timeout: Duration::from_millis(100),
        max_audio_bytes: 1024,
    })
    .unwrap();
    let backend = Backend::OpenAiCompatible {
        client,
        allow_upload: false,
    };
    let mut req = request(&path);
    req.silk = b"invalid";
    req.account = "";
    let error = transcribe_cached(&req, &backend).err().unwrap();
    assert_eq!(
        error.to_string(),
        "audio upload requires explicit authorization"
    );
    assert_eq!(fs::read(&path).unwrap(), b"broken-secret-marker");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn backend_failure_and_corrupt_cache_keep_separate_error_models() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model.bin");
    let path = dir.path().join("cache.json");
    fs::write(&model, "fail").unwrap();
    let backend = Backend::WhisperCpp(local::LocalConfig::new(program().to_owned(), model.clone()));
    let req = request(&path);
    let error = transcribe_cached(&req, &backend).err().unwrap();
    assert!(!format!("{error:#}").contains("SYNTHETIC_SECRET"));
    assert!(!path.exists());
    fs::write(&model, "ok").unwrap();
    fs::write(&path, b"{corrupt").unwrap();
    let result = transcribe_cached(&req, &backend).unwrap();
    assert_eq!(result.cache_state, CacheState::ReadUnavailable);
    assert_eq!(result.transcription.text, "synthetic ok");
    assert_eq!(fs::read(&path).unwrap(), b"{corrupt");
}

#[cfg(windows)]
#[test]
fn backend_cannot_modify_pinned_model_and_wrong_model_never_hits() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model.bin");
    let path = dir.path().join("cache.json");
    fs::write(&model, "probe").unwrap();
    let backend = Backend::WhisperCpp(local::LocalConfig::new(program().to_owned(), model.clone()));
    let req = request(&path);
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    assert_eq!(fs::read_to_string(&model).unwrap(), "probe");
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Hit
    );
    fs::write(&model, "fail").unwrap();
    assert!(transcribe_cached(&req, &backend).is_err());
}

#[test]
fn malformed_hit_or_wrong_timestamp_preserves_record_and_returns_fresh_success() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model.bin");
    let path = dir.path().join("cache.json");
    fs::write(&model, "ok").unwrap();
    let backend = Backend::WhisperCpp(local::LocalConfig::new(program().to_owned(), model));
    let req = request(&path);
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let key = original["entries"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    for entry in [
        serde_json::json!({"text":"forged stale text","language":"zh","create_time":999}),
        serde_json::json!({"text":123,"language":"zh","create_time":10,"unknown":"keep"}),
    ] {
        let mut altered = original.clone();
        altered["entries"][&key] = entry;
        let bytes = serde_json::to_vec(&altered).unwrap();
        fs::write(&path, &bytes).unwrap();
        let result = transcribe_cached(&req, &backend).unwrap();
        assert_eq!(result.cache_state, CacheState::ReadUnavailable);
        assert_eq!(result.transcription.text, "synthetic ok");
        assert_eq!(result.create_time, 10);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn existing_cache_never_bypasses_audio_validation_or_input_limit() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model.bin");
    let path = dir.path().join("cache.json");
    fs::write(&model, "ok").unwrap();
    let backend = Backend::WhisperCpp(local::LocalConfig::new(program().to_owned(), model));
    let mut req = request(&path);
    transcribe_cached(&req, &backend).unwrap();
    let before = fs::read(&path).unwrap();
    let oversized = vec![0u8; 16 * 1024 * 1024 + 1];
    for (audio, message) in [
        (b"invalid".as_slice(), "not a SILK_V3 file"),
        (oversized.as_slice(), "SILK input exceeds 16 MiB limit"),
    ] {
        req.silk = audio;
        let error = transcribe_cached(&req, &backend).err().unwrap();
        assert_eq!(error.to_string(), message);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
