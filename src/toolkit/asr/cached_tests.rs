use super::*;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::OnceLock,
};
const SILK: &[u8] = include_bytes!("../../../tests/fixtures/audio/silence.silk");

#[path = "receipt_cached_tests.rs"]
mod receipt_tests;

#[test]
fn program_content_change_invalidates_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), "ok");
    let program = dir.path().join("local-copy.exe");
    fs::copy(executable(), &program).unwrap();
    let Backend::Local(config) = &mut backend else {
        panic!()
    };
    config.executable = program.clone();
    let path = dir.path().join("cache.json");
    let req = request(&path);
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    fs::OpenOptions::new()
        .append(true)
        .open(program)
        .unwrap()
        .write_all(b"synthetic PE overlay")
        .unwrap();
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    assert_eq!(calls(dir.path()), 2);
}

#[test]
fn cloud_upload_hit_identity_isolation_and_authorization() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = count.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    // 有界回环服务读取完整 multipart 后才计数；退出后仍检查多余上传。
    let server = std::thread::spawn(move || {
        use std::{
            io::Read,
            sync::atomic::Ordering,
            time::{Duration, Instant},
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(Instant::now() < deadline, "loopback deadline exceeded");
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Windows 连接可能继承监听器的非阻塞模式；重试不重置期限。
                    stream.set_nonblocking(false).unwrap();
                    let read_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut buffer = [0u8; 4096];
                    loop {
                        let remaining = read_deadline.saturating_duration_since(Instant::now());
                        assert!(
                            !remaining.is_zero(),
                            "loopback request read deadline exceeded"
                        );
                        stream.set_read_timeout(Some(remaining)).unwrap();
                        let n = match stream.read(&mut buffer) {
                            Ok(n) => n,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                std::thread::sleep(Duration::from_millis(1));
                                continue;
                            }
                            Err(e) => panic!("loopback request read failed: {e}"),
                        };
                        assert!(n > 0);
                        bytes.extend_from_slice(&buffer[..n]);
                        assert!(bytes.len() < 1024 * 1024);
                        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let len: usize = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse().unwrap())
                                })
                                .unwrap();
                            if bytes.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    assert!(bytes.windows(4).any(|v| v == b"RIFF"));
                    observed.fetch_add(1, Ordering::SeqCst);
                    let body = r#"{"text":"synthetic cloud text","language":"zh"}"#;
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    assert!(Instant::now() < deadline, "loopback deadline exceeded");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let make = |endpoint: &str, model: &str, language: Option<&str>, key: &str, limit| {
        Backend::explicit_openai(
            super::super::openai::OpenAiConfig {
                base_url: format!("{base}/{endpoint}"),
                model: model.into(),
                language: language.map(str::to_owned),
                api_key: key.into(),
                timeout: std::time::Duration::from_secs(3),
                max_audio_bytes: limit,
            },
            true,
        )
        .unwrap()
    };
    let limit = super::super::openai::OPENAI_AUDIO_LIMIT_BYTES;
    let key = "SYNTHETIC_KEY_MUST_NOT_PERSIST";
    let mut backend = make("v1", "synthetic-model", Some("zh"), key, limit);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let mut req = request(&path);
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    let hit = transcribe_cached(&req, &backend).unwrap();
    assert_eq!(hit.cache_state, CacheState::Hit);
    assert_eq!(hit.create_time, req.create_time);
    assert_eq!(hit.transcription.text, "synthetic cloud text");
    let rotated = make("v1", "synthetic-model", Some("zh"), "ROTATED_SECRET", limit);
    assert_eq!(
        transcribe_cached(&req, &rotated).unwrap().cache_state,
        CacheState::Hit
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    let before = fs::read(&path).unwrap();
    let Backend::ExplicitOpenAi { allow_upload, .. } = &mut backend else {
        panic!()
    };
    *allow_upload = false;
    assert!(transcribe_cached(&req, &backend)
        .err()
        .unwrap()
        .to_string()
        .contains("authorization"));
    req.silk = b"invalid";
    req.account = "";
    let error = transcribe_cached(&req, &backend).err().unwrap();
    assert!(error.to_string().contains("authorization"));
    assert_eq!(fs::read(&path).unwrap(), before);
    req.account = "account-a";
    req.silk = SILK;
    for changed in [
        make("v2", "synthetic-model", Some("zh"), key, limit),
        make("v1", "other-model", Some("zh"), key, limit),
        make("v1", "synthetic-model", None, key, limit),
        make("v1", "synthetic-model", Some("en"), key, limit),
        make("v1", "synthetic-model", Some("zh"), key, limit - 1),
    ] {
        assert_eq!(
            transcribe_cached(&req, &changed).unwrap().cache_state,
            CacheState::Stored
        );
        assert_eq!(
            transcribe_cached(&req, &changed).unwrap().cache_state,
            CacheState::Hit
        );
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    server.join().unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 6);
    let persisted = fs::read_to_string(path).unwrap();
    for secret in [key, "ROTATED_SECRET", base.as_str(), "synthetic-model"] {
        assert!(!persisted.contains(secret));
    }
}

#[test]
fn oversized_input_is_rejected_before_container_or_model_access() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::Local(super::super::local::LocalConfig::new(
        dir.path().join("missing.exe"),
        dir.path().join("missing.bin"),
    ));
    let path = dir.path().join("cache.json");
    let mut req = request(&path);
    let bytes = vec![0; super::super::MAX_AUDIO_BYTES + 1];
    req.silk = &bytes;
    assert!(transcribe_cached(&req, &backend)
        .err()
        .unwrap()
        .to_string()
        .contains("size limit"));
    assert!(!path.exists());
    assert_eq!(
        serde_json::to_string(&CacheState::ReadUnavailable).unwrap(),
        "\"read_unavailable\""
    );
}

fn executable() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE.get_or_init(|| {
        let dir=tempfile::tempdir().unwrap(); let exe=dir.path().join("cached fake.exe");
        let mut cmd=Command::new("rustc");
        cmd.args(["--edition=2021","--crate-name","cached_fixture","-"]).arg("-o").arg(&exe)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
        let mut child=cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(br#"
use std::{env,fs,io::Write,path::PathBuf};
fn main(){let a:Vec<String>=env::args().collect();let arg=|k:&str| &a[a.iter().position(|v|v==k).unwrap()+1];
let model=PathBuf::from(arg("-m"));let mode=fs::read_to_string(&model).unwrap();
let mut counter=fs::OpenOptions::new().create(true).append(true).open(model.with_extension("calls")).unwrap();counter.write_all(b"x").unwrap();
if mode=="fail" {eprintln!("SYNTHETIC_CREDENTIAL_AUDIO");std::process::exit(7);}
let text=if mode=="empty" {""}else{"synthetic text"};
fs::write(PathBuf::from(arg("-of")).with_extension("txt"),text).unwrap();}
"#).unwrap();
        let output=child.wait_with_output().unwrap(); assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        (dir,exe)
    }).1
}
fn backend(dir: &Path, mode: &str) -> Backend {
    let model = dir.join("same-name.bin");
    fs::write(&model, mode).unwrap();
    Backend::Local(super::super::local::LocalConfig::new(
        executable().to_owned(),
        model,
    ))
}
fn request(path: &Path) -> CachedRequest<'_> {
    CachedRequest {
        cache_path: path,
        account: "account-a",
        username: "alice",
        source: "message/media_0.db",
        local_id: 1,
        create_time: 100,
        silk: SILK,
    }
}
fn calls(dir: &Path) -> usize {
    fs::read(dir.join("same-name.calls")).unwrap().len()
}

#[test]
fn hit_including_empty_success_calls_backend_once() {
    for mode in ["ok", "empty"] {
        let dir = tempfile::tempdir().unwrap();
        let backend = backend(dir.path(), mode);
        let path = dir.path().join("cache.json");
        let req = request(&path);
        assert_eq!(
            transcribe_cached(&req, &backend).unwrap().cache_state,
            CacheState::Stored
        );
        let hit = transcribe_cached(&req, &backend).unwrap();
        assert_eq!(hit.cache_state, CacheState::Hit);
        assert_eq!(calls(dir.path()), 1);
        assert_eq!(hit.transcription.text.is_empty(), mode == "empty");
    }
}

#[test]
fn account_audio_and_same_basename_model_changes_do_not_hit() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(dir.path(), "ok");
    let path = dir.path().join("cache.json");
    let mut req = request(&path);
    transcribe_cached(&req, &backend).unwrap();
    let original = fs::read(&path).unwrap();
    req.account = "account-b";
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::ReadUnavailable
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    req.account = "account-a";
    req.silk = include_bytes!("../../../tests/fixtures/audio/tone.silk");
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    fs::write(dir.path().join("same-name.bin"), "new-model").unwrap();
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    assert_eq!(calls(dir.path()), 4);
}

#[test]
fn operational_timeout_does_not_invalidate_completed_local_transcription() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), "ok");
    let path = dir.path().join("cache.json");
    let req = request(&path);
    assert_eq!(
        transcribe_cached(&req, &backend).unwrap().cache_state,
        CacheState::Stored
    );
    for timeout in [
        std::time::Duration::from_secs(2),
        std::time::Duration::from_millis(10),
    ] {
        if let Backend::Local(config) = &mut backend {
            config.timeout = timeout;
        }
        assert_eq!(
            transcribe_cached(&req, &backend).unwrap().cache_state,
            CacheState::Hit
        );
    }
    assert_eq!(calls(dir.path()), 1);
    if let Backend::Local(config) = &mut backend {
        config.timeout = std::time::Duration::ZERO;
    }
    assert!(transcribe_cached(&req, &backend).is_err());
    assert_eq!(calls(dir.path()), 1);
}

#[test]
fn failure_never_creates_or_changes_cache() {
    let dir = tempfile::tempdir().unwrap();
    let failed = backend(dir.path(), "fail");
    let path = dir.path().join("cache.json");
    let req = request(&path);
    let error = transcribe_cached(&req, &failed).err().unwrap();
    assert!(!format!("{error:#}").contains("SYNTHETIC_CREDENTIAL_AUDIO"));
    assert!(!path.exists());
    let good = backend(dir.path(), "ok");
    assert_eq!(
        transcribe_cached(&req, &good).unwrap().cache_state,
        CacheState::Stored
    );
    let before = fs::read(&path).unwrap();
    let failed = backend(dir.path(), "fail");
    let error = transcribe_cached(&req, &failed).err().unwrap();
    assert!(!format!("{error:#}").contains("SYNTHETIC_CREDENTIAL_AUDIO"));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!String::from_utf8(before)
        .unwrap()
        .contains("SYNTHETIC_CREDENTIAL_AUDIO"));
    assert_eq!(calls(dir.path()), 3);
    assert!(!dir.path().join(".cache.json.asr-cache.lock").exists());
}

#[test]
fn cache_write_failure_keeps_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let req = request(&path);
    let good = backend(dir.path(), "ok");
    fs::write(
        dir.path().join(".cache.json.asr-cache.lock"),
        b"synthetic lock",
    )
    .unwrap();
    let result = transcribe_cached(&req, &good).unwrap();
    assert_eq!(result.cache_state, CacheState::WriteUnavailable);
    assert_eq!(result.transcription.text, "synthetic text");
    assert!(!path.exists());
}

#[test]
fn checked_cache_rejection_never_publishes_and_hits_do_not_commit() {
    for reject_at in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let req = request(&path);
        let good = backend(dir.path(), "ok");
        let mut checks = 0;
        let result = transcribe_cached_checked(&req, &good, |transcription| {
            assert_eq!(transcription.text, "synthetic text");
            checks += 1;
            anyhow::ensure!(checks != reject_at, "synthetic rejection");
            Ok(())
        });
        assert_eq!(checks, reject_at);
        if reject_at == 1 {
            assert!(result.is_err());
        } else {
            // 写入阶段仍可降级；需要拒绝响应的宿主应独立保留回调错误。
            assert_eq!(result.unwrap().cache_state, CacheState::WriteUnavailable);
        }
        assert!(!path.exists());
        assert!(!dir.path().join(".cache.json.asr-cache.lock").exists());
        assert!(!fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".wx-cache-")));
        assert_eq!(calls(dir.path()), 1);
        assert_eq!(
            transcribe_cached(&req, &good).unwrap().cache_state,
            CacheState::Stored
        );
        let hit = transcribe_cached_checked(&req, &good, |_| {
            panic!("cache hit must not invoke publication callback")
        })
        .unwrap();
        assert_eq!(hit.cache_state, CacheState::Hit);
        assert_eq!(calls(dir.path()), 2);
    }
}

#[test]
fn invalid_local_configuration_cannot_use_old_hit() {
    let dir = tempfile::tempdir().unwrap();
    let mut backend = backend(dir.path(), "ok");
    let path = dir.path().join("cache.json");
    let req = request(&path);
    transcribe_cached(&req, &backend).unwrap();
    let Backend::Local(config) = &mut backend else {
        panic!()
    };
    config.threads = 0;
    assert!(transcribe_cached(&req, &backend).is_err());
    assert_eq!(calls(dir.path()), 1);
}
