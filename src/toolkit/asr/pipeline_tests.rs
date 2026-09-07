use super::*;
use clap::Parser;

use crate::cli::asr as cli;

#[derive(Parser)]
struct AudioCli {
    #[command(flatten)]
    args: cli::TranscribeAudioNativeArgs,
}

#[derive(Parser)]
struct ChatCli {
    #[command(flatten)]
    args: cli::TranscribeChatNativeArgs,
}

#[test]
fn cli_registration_contract() {
    use clap::CommandFactory;
    AudioCli::command().debug_assert();
    ChatCli::command().debug_assert();
    let _: fn(cli::TranscribeAudioNativeArgs) -> Result<()> = cli::cmd_transcribe_audio_native;
    let _: fn(cli::TranscribeChatNativeArgs) -> Result<()> = cli::cmd_transcribe_chat_native;
}

#[test]
fn pcm_header_and_wav_roundtrip() {
    let pcm = [0, 0, 255, 127, 0, 128];
    let wav = pcm24k_to_wav(&pcm).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[24..28], &24_000u32.to_le_bytes());
    assert_eq!(&wav[44..], &pcm);
    validate_wav(&wav).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.bin");
    fs::write(&path, &wav).unwrap();
    assert_eq!(prepare_wav(&path).unwrap(), wav);
    assert!(pcm24k_to_wav(&[]).is_err());
    assert!(pcm24k_to_wav(&[0]).is_err());
}

#[test]
fn corrupt_wav_is_rejected() {
    let wav = pcm24k_to_wav(&[0; 8]).unwrap();
    for index in [4, 8, 20, 22, 28, 32, 34, 40] {
        let mut corrupt = wav.clone();
        corrupt[index] = 255;
        assert!(validate_wav(&corrupt).is_err(), "index {index}");
    }
    assert!(validate_wav(&wav[..20]).is_err());
}

#[test]
fn native_silk_fixture_decodes_to_wav() {
    let silk = include_bytes!("../../../tests/fixtures/audio/silence.silk");
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("silence.silk");
    fs::write(&input, silk).unwrap();
    let wav = prepare_wav(&input).unwrap();
    validate_wav(&wav).unwrap();
    assert_eq!(
        &wav[44..],
        crate::toolkit::audio::decode_silk_to_pcm(silk).unwrap()
    );
}

#[test]
fn upload_denied_before_input_or_key_access() {
    let parsed = AudioCli::try_parse_from([
        "wx",
        "missing.wav",
        "--backend",
        "explicit-open-ai",
        "--api-key-file",
        "missing.key",
    ])
    .unwrap();
    let error = parsed.args.backend.build().unwrap_err().to_string();
    assert!(error.contains("--allow-upload"));
    let config = openai::OpenAiConfig {
        base_url: "invalid".into(),
        model: String::new(),
        language: None,
        api_key: String::new(),
        timeout: std::time::Duration::ZERO,
        max_audio_bytes: 0,
    };
    assert!(Backend::explicit_openai(config, false)
        .unwrap_err()
        .to_string()
        .contains("authorization"));
}

#[test]
fn cli_local_configuration_and_no_cloud_defaults() {
    let args = AudioCli::try_parse_from([
        "wx",
        "audio.wav",
        "--whisper-binary",
        "whisper.exe",
        "--whisper-model",
        "model.bin",
        "--threads",
        "3",
    ])
    .unwrap();
    let Backend::Local(config) = args.args.backend.build().unwrap() else {
        panic!()
    };
    assert_eq!(config.threads, 3);
    assert_eq!(config.output_format, local::OutputFormat::Json);
    let args = AudioCli::try_parse_from([
        "wx",
        "audio.wav",
        "--backend",
        "explicit-open-ai",
        "--allow-upload",
    ])
    .unwrap();
    assert!(args
        .args
        .backend
        .build()
        .unwrap_err()
        .to_string()
        .contains("--openai-base-url"));
}

fn manifest(dir: &Path, entries: serde_json::Value) -> PathBuf {
    let path = dir.join("media.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"entries": entries})).unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn media_identity_is_exact_and_duplicates_fail() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.wav"), pcm24k_to_wav(&[0; 4]).unwrap()).unwrap();
    let entry =
        serde_json::json!({"username":"u","source":"message_0.db","local_id":1,"audio":"a.wav"});
    let file = manifest(dir.path(), serde_json::json!([entry]));
    let media = OfflineMedia::from_manifest(&file, dir.path()).unwrap();
    let mut id = writeback::VoiceIdentity {
        username: "u".into(),
        source: "message_0.db".into(),
        local_id: 1,
    };
    assert!(media.resolve(&id).is_ok());
    id.source = "message_1.db".into();
    assert!(media.resolve(&id).is_err());
    id.source = "message_0.db".into();
    id.username = "other".into();
    assert!(media.resolve(&id).is_err());
    let file = manifest(dir.path(), serde_json::json!([entry, entry]));
    assert!(OfflineMedia::from_manifest(&file, dir.path())
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
}

#[test]
fn media_rejects_traversal_and_unknown_source() {
    let dir = tempfile::tempdir().unwrap();
    for audio in ["../outside.wav", "C:\\outside.wav", "a.wav:stream"] {
        let file = manifest(
            dir.path(),
            serde_json::json!([{"username":"u","source":"s","local_id":1,"audio":audio}]),
        );
        assert!(OfflineMedia::from_manifest(&file, dir.path()).is_err());
    }
    let file = manifest(
        dir.path(),
        serde_json::json!([{"username":"u","source":"unknown","local_id":1,"audio":"a.wav"}]),
    );
    assert!(OfflineMedia::from_manifest(&file, dir.path()).is_err());
}

#[test]
fn chat_unknown_source_saved_as_failure_not_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let file = manifest(dir.path(), serde_json::json!([]));
    let media = OfflineMedia::from_manifest(&file, dir.path()).unwrap();
    let input = dir.path().join("chat.json");
    let output = dir.path().join("out.json");
    let data = serde_json::json!({"username":"u","messages":[{"type":"voice","source":"missing","local_id":1}]});
    fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
    let backend = Backend::Local(local::LocalConfig::new(
        "absent.exe".into(),
        "absent.bin".into(),
    ));
    let report = transcribe_chat(&input, &output, &media, &backend).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(output).unwrap()).unwrap(),
        data
    );
}

#[test]
fn local_pipeline_and_chat_writeback_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("fake whisper.exe");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asr-local/fake.rs");
    let mut command = std::process::Command::new("rustc");
    command
        .arg("--edition=2021")
        .arg(source)
        .arg("-o")
        .arg(&exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let compile = command.output().unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let model = dir.path().join("model.bin");
    fs::write(&model, "ok").unwrap();
    let mut config = local::LocalConfig::new(exe, model);
    config.language = "zh".into();
    config.threads = 2;
    config.output_format = local::OutputFormat::Json;
    config.temp_root = Some(dir.path().to_owned());
    let backend = Backend::Local(config);
    let audio = dir.path().join("audio.wav");
    let wav = pcm24k_to_wav(&[0; 48]).unwrap();
    fs::write(&audio, &wav).unwrap();
    let count = fs::read_dir(dir.path()).unwrap().count();
    let result = transcribe_audio(&audio, &backend).unwrap();
    assert_eq!(result.text, "hello world");
    assert_eq!(result.backend, "whisper_cpp");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), count);
    assert_eq!(fs::read(&audio).unwrap(), wav);
    let silk = dir.path().join("audio.silk");
    fs::write(
        &silk,
        include_bytes!("../../../tests/fixtures/audio/silence.silk"),
    )
    .unwrap();
    assert_eq!(
        transcribe_audio(&silk, &backend).unwrap().text,
        "hello world"
    );

    let file = manifest(
        dir.path(),
        serde_json::json!([
            {"username":"u","source":"s0","local_id":1,"audio":"audio.wav"},
            {"username":"u","source":"s1","local_id":1,"audio":"audio.silk"}
        ]),
    );
    let media = OfflineMedia::from_manifest(&file, dir.path()).unwrap();
    let input = dir.path().join("chat.json");
    let output = dir.path().join("out.json");
    let data = serde_json::json!({"username":"u","messages":[
        {"type":"voice","source":"s0","local_id":1},
        {"type":"voice","source":"s1","local_id":1},
        {"type":"voice","source":"s0","local_id":2,"transcription":"keep"}
    ]});
    fs::write(&input, serde_json::to_vec(&data).unwrap()).unwrap();
    let report = transcribe_chat(&input, &output, &media, &backend).unwrap();
    assert_eq!(report.transcribed, 2);
    assert_eq!(report.skipped_existing, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&input).unwrap()).unwrap(),
        data
    );
    let report = transcribe_chat(&output, &output, &media, &backend).unwrap();
    assert_eq!(report.skipped_existing, 3);
}

#[test]
fn explicit_cloud_pipeline_uploads_wav_to_loopback_only() {
    use std::{
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(start.elapsed() < Duration::from_secs(5));
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        };
        // Windows 接受的连接可能继承监听套接字的非阻塞模式。
        stream.set_nonblocking(false).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let read_deadline = Instant::now() + Duration::from_secs(5);
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let remaining = read_deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "loopback request read deadline exceeded"
            );
            stream.set_read_timeout(Some(remaining)).unwrap();
            let n = match stream.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(e) => panic!("loopback request read failed: {e}"),
            };
            assert_ne!(n, 0);
            request.extend_from_slice(&chunk[..n]);
            if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let response = r#"{"text":"cloud fixture","language":"zh"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
        request
    });
    let backend = Backend::explicit_openai(
        openai::OpenAiConfig {
            base_url: url,
            model: "synthetic-model".into(),
            api_key: "synthetic-key".into(),
            language: None,
            timeout: Duration::from_secs(5),
            max_audio_bytes: openai::OPENAI_AUDIO_LIMIT_BYTES,
        },
        true,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("synthetic.silk");
    fs::write(
        &input,
        include_bytes!("../../../tests/fixtures/audio/silence.silk"),
    )
    .unwrap();
    let expected = prepare_wav(&input).unwrap();
    let result = transcribe_audio(&input, &backend);
    let request = server.join().unwrap();
    let result = result.unwrap();
    assert_eq!(result.text, "cloud fixture");
    assert_eq!(result.backend, "openai");
    assert!(request
        .windows(expected.len())
        .any(|bytes| bytes == expected));
}
