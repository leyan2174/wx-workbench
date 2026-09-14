//! 仅运行 security_ 前缀测试；所有音频、凭据和网络响应均为本机合成数据。
#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "fixtures/asr-video-security/modules.rs"]
#[allow(dead_code)] // harness 仅审查安全边界，不调用每个生产入口。
mod production;
use bootstrap::BootstrapCleanup;
use production::{asr, video};
pub use production::{
    attachment, cli, config, crypto, daemon, ipc, key_store, runtime, toolkit, windows_process,
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const SECRET: &str = "SYNTHETIC_CREDENTIAL_MUST_NOT_LEAK";

fn repo() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if root.join("src/main.rs").exists() {
        root
    } else {
        root.join("../../..")
    }
}

fn config(url: String) -> asr::openai::OpenAiConfig {
    asr::openai::OpenAiConfig {
        base_url: url,
        model: "synthetic-model".into(),
        language: None,
        api_key: SECRET.into(),
        timeout: Duration::from_secs(2),
        max_audio_bytes: 1024,
    }
}

fn listener() -> TcpListener {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    server
}

fn read_request(listener: TcpListener) -> (TcpStream, Vec<u8>) {
    let start = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(start.elapsed() < Duration::from_secs(5), "mock 未收到请求");
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e}"),
        }
    };
    // Windows 接受的连接可能继承监听器的非阻塞模式。
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).unwrap();
        assert!(
            n > 0 && bytes.len() + n <= 64 * 1024,
            "mock 请求超限或提前关闭"
        );
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let len: usize = header
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + len {
                break;
            }
        }
    }
    (stream, bytes)
}

#[test]
fn security_cloud_denial_precedes_audio_access_and_has_zero_requests() {
    let server = listener();
    let client = asr::openai::OpenAiTranscriber::new(config(format!(
        "http://{}/v1",
        server.local_addr().unwrap()
    )))
    .unwrap();
    let backend = asr::Backend::ExplicitOpenAi {
        client,
        allow_upload: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let error = asr::transcribe_audio(&dir.path().join("missing.wav"), &backend).unwrap_err();
    assert!(error.to_string().contains("authorization"));
    assert_eq!(
        server.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!format!("{backend:?} {error:#}").contains(SECRET));
}

#[test]
fn security_cloud_redirect_does_not_forward_credentials_or_audio() {
    let server = listener();
    let destination = listener();
    let target = format!("http://{}/stolen", destination.local_addr().unwrap());
    let client = asr::openai::OpenAiTranscriber::new(config(format!(
        "http://{}/v1",
        server.local_addr().unwrap()
    )))
    .unwrap();
    let mock = thread::spawn(move || {
        let (mut stream, request) = read_request(server);
        write!(stream, "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        request
    });
    let error = client.transcribe_wav(b"SYNTHETIC_AUDIO", true).unwrap_err();
    let request = mock.join().unwrap();
    assert!(String::from_utf8_lossy(&request).contains(SECRET));
    assert_eq!(
        error,
        asr::openai::OpenAiError::Http {
            status: 307,
            json_error: false
        }
    );
    assert_eq!(
        destination.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!format!("{error:?} {error}").contains(SECRET));
}

#[test]
fn security_cloud_error_body_is_redacted_and_unframed_response_is_bounded() {
    for oversized in [false, true] {
        let server = listener();
        let client = asr::openai::OpenAiTranscriber::new(config(format!(
            "http://{}/v1",
            server.local_addr().unwrap()
        )))
        .unwrap();
        let mock = thread::spawn(move || {
            let (mut stream, _) = read_request(server);
            // 不发 Content-Length，验证实际读流限额，而非仅检查响应头。
            stream
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n")
                .unwrap();
            let body = if oversized {
                vec![b'x'; 1024 * 1024 + 2]
            } else {
                format!(r#"{{"error":"{SECRET} SYNTHETIC_AUDIO"}}"#).into_bytes()
            };
            let _ = stream.write_all(&body);
        });
        let error = client.transcribe_wav(b"SYNTHETIC_AUDIO", true).unwrap_err();
        mock.join().unwrap();
        assert_eq!(
            error,
            if oversized {
                asr::openai::OpenAiError::ResponseTooLarge
            } else {
                asr::openai::OpenAiError::Http {
                    status: 401,
                    json_error: true,
                }
            }
        );
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(SECRET) && !rendered.contains("SYNTHETIC_AUDIO"));
    }
}

#[test]
fn security_cli_cloud_authorization_and_key_limit_precede_upload() {
    let dir = tempfile::tempdir().unwrap();
    let _bootstrap = BootstrapCleanup(dir.path().join("runtime"));
    let key = dir.path().join("key.txt");
    let audio = dir.path().join("audio.wav");
    fs::write(&audio, asr::pcm24k_to_wav(&[0; 8]).unwrap()).unwrap();
    let server = listener();
    let url = format!("http://{}/v1", server.local_addr().unwrap());
    let base = [
        "toolkit",
        "transcribe-audio-native",
        audio.to_str().unwrap(),
        "--backend",
        "explicit-open-ai",
        "--openai-base-url",
        &url,
        "--openai-model",
        "synthetic",
        "--api-key-file",
        key.to_str().unwrap(),
    ];
    // key 尚不存在；授权错误必须优先于凭据或音频读取错误。
    let error = process_failure(run_wx(dir.path(), &base));
    assert!(error.contains("allow-upload"));
    assert!(!dir.path().join("runtime").exists());
    fs::write(&key, SECRET.repeat(600)).unwrap();
    let mut authorized = base.to_vec();
    authorized.push("--allow-upload");
    let error = process_failure(run_wx(dir.path(), &authorized));
    assert!(error.contains("limit"));
    assert_eq!(
        server.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    fs::write(&key, SECRET).unwrap();
    let mock = thread::spawn(move || {
        let (mut stream, request) = read_request(server);
        let body = format!(r#"{{"error":"{SECRET}"}}"#);
        write!(
            stream,
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        request
    });
    let result = run_wx(dir.path(), &authorized);
    let request = mock.join().unwrap();
    let error = process_failure(result);
    assert!(error.contains("401"));
    assert!(String::from_utf8_lossy(&request).contains(SECRET));
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
        .env("WX_WECHAT_DECRYPT_PYTHON", root.join("absent-python.exe"))
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
                "toolkit",
                "decode-sns-video",
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
            "toolkit",
            "decode-sns-video",
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
                "toolkit",
                "decode-sns-video",
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

fn chat() -> serde_json::Value {
    serde_json::json!({"username":"alice","messages":[{"type":"voice","source":"message_0.db","local_id":1}]})
}

#[test]
fn security_writeback_rechecks_owner_after_transcription_before_publish() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let output = dir.path().join("output.json");
    let original = serde_json::to_vec(&chat()).unwrap();
    fs::write(&input, &original).unwrap();
    fs::write(&output, &original).unwrap();
    let foreign = br#"{"username":"bob","messages":[],"sentinel":"FOREIGN_ACCOUNT"}"#;
    let result = asr::writeback::transcribe_file(&input, &output, |_| {
        // 确定性模拟另一任务在漫长识别期间发布其他账号导出。
        fs::write(&output, foreign).unwrap();
        Ok("synthetic recognized text".into())
    });
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(
        fs::read(&output).unwrap(),
        foreign,
        "转录开始时的身份校验不能授权覆盖随后发布的其他账号文件；result={result:?}"
    );
    assert!(result.is_err());
}

#[test]
fn security_local_diagnostic_does_not_leak_audio_into_writeback_report() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("fake.exe");
    let source = repo().join("tests/fixtures/asr-video-security/fake_diagnostic.rs");
    println!("rustc --edition=2021 {:?} -o {:?}", source, executable);
    let compile = std::process::Command::new("rustc")
        .arg("--edition=2021")
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    println!(
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(compile.status.success());
    let model = dir.path().join("model.bin");
    let audio = dir.path().join("audio.wav");
    fs::write(&model, b"synthetic").unwrap();
    fs::write(&audio, b"SYNTHETIC_PRIVATE_AUDIO_PAYLOAD").unwrap();
    let mut config = asr::local::LocalConfig::new(executable, model);
    config.temp_root = Some(dir.path().to_owned());
    config.timeout = Duration::from_secs(3);
    let before = fs::read_dir(dir.path()).unwrap().count();
    let mut data = chat();
    let report = asr::writeback::transcribe_json(&mut data, |_| {
        Ok(asr::local::transcribe(&config, &audio)?.text)
    })
    .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(data, chat());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before);
    assert!(
        !format!("{report:?}").contains("SYNTHETIC_PRIVATE_AUDIO_PAYLOAD"),
        "原始音频诊断被写入上层错误报告: {report:?}"
    );
}

#[test]
fn security_video_rejects_tampered_assets_and_recovers_after_bad_key() {
    let dir = tempfile::tempdir().unwrap();
    let asset = repo().join("vendor/wechat-decrypt/sns_media_wasm/wasm_video_decode.wasm");
    let original = fs::read(&asset).unwrap();
    let tampered = dir.path().join("tampered.wasm");
    let mut bytes = original.clone();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&tampered, &bytes).unwrap();
    assert!(matches!(
        video::VideoRuntime::new(&tampered, video::RuntimeLimits::default()),
        Err(video::VideoRuntimeError::UnsupportedAsset)
    ));
    let huge = dir.path().join("oversized.wasm");
    fs::File::create(&huge)
        .unwrap()
        .set_len(4 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        video::VideoRuntime::new(&huge, video::RuntimeLimits::default()),
        Err(video::VideoRuntimeError::UnsupportedAsset)
    ));
    let runtime = video::VideoRuntime::new(&asset, video::RuntimeLimits::default()).unwrap();
    let expected = runtime.keystream("42", 16).unwrap();
    for key in [SECRET.to_owned(), "1".repeat(1025)] {
        let error = runtime.keystream(&key, 16).unwrap_err();
        assert!(!format!("{error:?} {error}").contains(SECRET));
        assert_eq!(runtime.keystream("42", 16).unwrap(), expected);
    }
    let invalid = vec![0; 64];
    assert_eq!(
        runtime.decode("42", &invalid),
        Err(video::VideoRuntimeError::InvalidMp4)
    );
    assert_eq!(invalid, vec![0; 64]);
    assert_eq!(fs::read(&asset).unwrap(), original);
}
