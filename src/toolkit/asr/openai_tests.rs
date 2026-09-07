use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const KEY: &str = "synthetic-secret-do-not-echo";
const AUDIO: &[u8] = b"RIFF\x00\xff\x80synthetic-WAVE\r\n";

fn config(url: String) -> OpenAiConfig {
    OpenAiConfig {
        base_url: url,
        model: "whisper-1".into(),
        language: Some("zh".into()),
        api_key: KEY.into(),
        timeout: Duration::from_secs(3),
        max_audio_bytes: OPENAI_AUDIO_LIMIT_BYTES,
    }
}

fn request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = stream.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + length {
                break;
            }
        }
    }
    bytes
}

fn mock(status: u16, body: Vec<u8>, delay: Duration) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let bytes = request(&mut stream);
        thread::sleep(delay);
        let headers = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nLocation: /must-not-follow\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(headers.as_bytes());
        let _ = stream.write_all(&body);
        bytes
    });
    (url, handle)
}

#[test]
fn multipart_contains_exact_audio_auth_and_fields() {
    let (url, server) = mock(
        200,
        br#"{"text":"  hello  ","language":"chinese"}"#.to_vec(),
        Duration::ZERO,
    );
    let client = OpenAiTranscriber::new(config(url)).unwrap();
    let result = client.transcribe_wav(AUDIO, true).unwrap();
    assert_eq!(result.text, "hello");
    assert_eq!(result.language, "chinese");
    let bytes = server.join().unwrap();
    let header_end = bytes.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
    assert!(headers.starts_with("post /v1/audio/transcriptions http/1.1\r\n"));
    assert!(headers.contains(&format!("authorization: bearer {KEY}\r\n")));
    let boundary = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-type: multipart/form-data; boundary="))
        .unwrap();
    let body = &bytes[header_end + 4..];
    let visible = String::from_utf8_lossy(body);
    assert!(visible.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"));
    assert!(visible.contains("name=\"language\"\r\n\r\nzh\r\n"));
    assert!(visible.contains("name=\"response_format\"\r\n\r\nverbose_json\r\n"));
    assert!(visible
        .contains("name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"));
    let start = body.windows(AUDIO.len()).position(|w| w == AUDIO).unwrap();
    assert!(body[start + AUDIO.len()..].starts_with(format!("\r\n--{boundary}").as_bytes()));
    assert!(body.ends_with(format!("--{boundary}--\r\n").as_bytes()));
    assert!(!visible.contains(KEY));
}

#[test]
fn optional_language_and_null_text_match_legacy() {
    let (url, server) = mock(200, br#"{"text":null}"#.to_vec(), Duration::ZERO);
    let mut cfg = config(url);
    cfg.language = None;
    let result = OpenAiTranscriber::new(cfg)
        .unwrap()
        .transcribe_wav(AUDIO, true)
        .unwrap();
    assert_eq!(result.text, "");
    assert_eq!(result.language, "unknown");
    assert!(!String::from_utf8_lossy(&server.join().unwrap()).contains("name=\"language\""));
}

#[test]
fn preflight_never_connects_without_consent_or_with_invalid_size() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let client =
        OpenAiTranscriber::new(config(format!("http://{}", listener.local_addr().unwrap())))
            .unwrap();
    assert_eq!(
        client.transcribe_wav(AUDIO, false),
        Err(OpenAiError::UploadNotAuthorized)
    );
    assert_eq!(
        client.transcribe_wav(&[], true),
        Err(OpenAiError::EmptyAudio)
    );
    assert_eq!(
        client.transcribe_wav(&vec![0; OPENAI_AUDIO_LIMIT_BYTES + 1], true),
        Err(OpenAiError::AudioTooLarge)
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn exact_custom_limit_is_accepted() {
    let (url, server) = mock(200, br#"{"text":"ok"}"#.to_vec(), Duration::ZERO);
    let mut cfg = config(url);
    cfg.max_audio_bytes = AUDIO.len();
    assert!(OpenAiTranscriber::new(cfg)
        .unwrap()
        .transcribe_wav(AUDIO, true)
        .is_ok());
    server.join().unwrap();
}

#[test]
fn errors_and_debug_never_echo_secrets() {
    for status in [401, 429, 500, 302] {
        let (url, server) = mock(
            status,
            format!("{{\"error\":{{\"message\":\"{KEY}\"}}}}").into_bytes(),
            Duration::ZERO,
        );
        let cfg = config(url);
        assert!(!format!("{cfg:?}").contains(KEY));
        let client = OpenAiTranscriber::new(cfg).unwrap();
        assert!(!format!("{client:?}").contains(KEY));
        let error = client.transcribe_wav(AUDIO, true).unwrap_err();
        assert_eq!(
            error,
            OpenAiError::Http {
                status,
                json_error: true
            }
        );
        assert!(!format!("{error:?} {error}").contains(KEY));
        assert!(std::error::Error::source(&error).is_none());
        server.join().unwrap();
    }
}

#[test]
fn malformed_success_and_non_json_http_error() {
    for body in [
        "not-json",
        "{}",
        "{\"text\":1}",
        "{\"text\":\"ok\",\"language\":3}",
    ] {
        let (url, server) = mock(200, body.as_bytes().to_vec(), Duration::ZERO);
        assert_eq!(
            OpenAiTranscriber::new(config(url))
                .unwrap()
                .transcribe_wav(AUDIO, true),
            Err(OpenAiError::InvalidResponse)
        );
        server.join().unwrap();
    }
    let (url, server) = mock(502, KEY.as_bytes().to_vec(), Duration::ZERO);
    assert_eq!(
        OpenAiTranscriber::new(config(url))
            .unwrap()
            .transcribe_wav(AUDIO, true),
        Err(OpenAiError::Http {
            status: 502,
            json_error: false
        })
    );
    server.join().unwrap();
}

#[test]
fn timeout_is_bounded_and_sanitized() {
    let (url, server) = mock(200, b"{}".to_vec(), Duration::from_millis(250));
    let mut cfg = config(url);
    cfg.timeout = Duration::from_millis(50);
    assert_eq!(
        OpenAiTranscriber::new(cfg)
            .unwrap()
            .transcribe_wav(AUDIO, true),
        Err(OpenAiError::Timeout)
    );
    server.join().unwrap();
}

#[test]
fn body_read_timeout_is_also_classified() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(format!("http://{}", listener.local_addr().unwrap()));
    cfg.timeout = Duration::from_millis(100);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
            .unwrap();
        thread::sleep(Duration::from_millis(300));
    });
    assert_eq!(
        OpenAiTranscriber::new(cfg)
            .unwrap()
            .transcribe_wav(AUDIO, true),
        Err(OpenAiError::Timeout)
    );
    server.join().unwrap();
}

#[test]
fn request_timeout_can_only_shrink_without_changing_cache_identity() {
    let (url, server) = mock(200, b"{}".to_vec(), Duration::from_millis(250));
    let mut client = OpenAiTranscriber::new(config(url)).unwrap();
    let before = format!("{:?}", client.cache_identity());
    client.tighten_timeout(Duration::from_millis(50)).unwrap();
    client.tighten_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(client.timeout, Duration::from_millis(50));
    assert_eq!(before, format!("{:?}", client.cache_identity()));
    assert_eq!(
        client.transcribe_wav(AUDIO, true),
        Err(OpenAiError::Timeout)
    );
    assert_eq!(
        client.tighten_timeout(Duration::ZERO),
        Err(OpenAiError::Timeout)
    );
    assert_eq!(client.timeout, Duration::from_millis(50));
    server.join().unwrap();
}

#[test]
fn compatible_base_path_and_model_are_explicit() {
    let (url, server) = mock(
        200,
        "{\"text\":\" 合成文本 \"}".as_bytes().to_vec(),
        Duration::ZERO,
    );
    let mut cfg = config(url.replace("/v1/", "/compatible/v2"));
    cfg.model = "compatible-transcriber".into();
    cfg.language = Some("en".into());
    let result = OpenAiTranscriber::new(cfg)
        .unwrap()
        .transcribe_wav(AUDIO, true)
        .unwrap();
    assert_eq!(result.text, "合成文本");
    let bytes = server.join().unwrap();
    let wire = String::from_utf8_lossy(&bytes);
    assert!(wire.starts_with("POST /compatible/v2/audio/transcriptions HTTP/1.1"));
    assert!(wire.contains("name=\"model\"\r\n\r\ncompatible-transcriber\r\n"));
    assert!(wire.contains("name=\"language\"\r\n\r\nen\r\n"));
}

#[test]
fn response_limit_is_enforced() {
    let (url, server) = mock(
        200,
        vec![b'x'; RESPONSE_LIMIT_BYTES as usize + 1],
        Duration::ZERO,
    );
    assert_eq!(
        OpenAiTranscriber::new(config(url))
            .unwrap()
            .transcribe_wav(AUDIO, true),
        Err(OpenAiError::ResponseTooLarge)
    );
    server.join().unwrap();
}

#[test]
fn invalid_config_does_not_echo_input() {
    for url in [
        "not a url",
        "http://example.com/v1",
        "https://user:secret@example.com",
        "https://example.com?secret=x",
        "https://example.com/#secret",
    ] {
        assert_eq!(
            OpenAiTranscriber::new(config(url.into())).unwrap_err(),
            OpenAiError::InvalidConfig
        );
    }
    for case in 0..6 {
        let mut cfg = config("https://example.com/v1".into());
        match case {
            0 => cfg.api_key = "".into(),
            1 => cfg.api_key = format!("{KEY}\r\nx: injected"),
            2 => cfg.model.clear(),
            3 => cfg.language = Some(" ".into()),
            4 => cfg.timeout = Duration::ZERO,
            _ => cfg.max_audio_bytes = OPENAI_AUDIO_LIMIT_BYTES + 1,
        }
        let error = OpenAiTranscriber::new(cfg).unwrap_err();
        assert_eq!(error, OpenAiError::InvalidConfig);
        assert!(!format!("{error:?} {error}").contains(KEY));
    }
}
