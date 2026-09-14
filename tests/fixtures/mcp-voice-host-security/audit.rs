use mcp_voice_host_security::{
    service::operation_requests::asr::{BackendArgs, BackendKind},
    config::Config,
    ipc::Response,
    mcp::protocol::{CallContext, DispatchError},
    mcp_voice::{Args, Operation},
    runtime::RuntimeContext,
    toolkit::asr::{
        database_media::{DatabaseVoice, VoiceEvidence},
        prepared_audio,
    },
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    root: tempfile::TempDir,
    rt: RuntimeContext,
    out: PathBuf,
    model: PathBuf,
    temp: PathBuf,
    cache: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        let out = root.path().join("output");
        let temp = root.path().join("temporary");
        let backend = root.path().join("backend");
        let cache = root.path().join("transcripts/voices.json");
        for p in [&account, &out, &temp, &backend, cache.parent().unwrap()] {
            fs::create_dir_all(p).unwrap();
        }
        let config = Config {
            key_store: None,
            db_dir: account.join("db"),
            keys_file: account.join("keys.json"),
            decrypted_dir: account.join("decrypted"),
            wechat_process: "Weixin.exe".into(),
        };
        fs::create_dir(&config.db_dir).unwrap();
        fs::create_dir(&config.decrypted_dir).unwrap();
        fs::write(&config.keys_file, b"{}").unwrap();
        let config_path = account.join("config.json");
        fs::write(&config_path, serde_json::to_vec(&json!({"db_dir":config.db_dir,"keys_file":config.keys_file,"decrypted_dir":config.decrypted_dir,"wechat_process":"Weixin.exe"})).unwrap()).unwrap();
        let rt = RuntimeContext {
            config,
            config_path,
            root: account.clone(),
            directory: account.join("runtime"),
            id: "synthetic-A".into(),
        };
        let model = backend.join("model.json");
        fs::write(&model, b"{}").unwrap();
        Self {
            root,
            rt,
            out,
            model,
            temp,
            cache,
        }
    }
    fn args(&self) -> Args {
        Args {
            backend: BackendArgs {
                whisper_binary: Some(env!("CARGO_BIN_EXE_synthetic-asr").into()),
                whisper_model: Some(self.model.clone()),
                temp_root: Some(self.temp.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn local_cli(&self) -> Vec<String> {
        vec![
            "--whisper-binary".into(),
            env!("CARGO_BIN_EXE_synthetic-asr").into(),
            "--whisper-model".into(),
            self.model.to_string_lossy().into(),
            "--temp-root".into(),
            self.temp.to_string_lossy().into(),
        ]
    }
    fn finish(
        &self,
        op: Operation,
        args: &Args,
        response: Response,
    ) -> Result<Response, DispatchError> {
        let ctx = CallContext::default();
        args.prepare(op, 42, Some(&self.out), &ctx)?
            .bind(&self.rt)?
            .finish(response, &self.rt, &ctx, || Ok(()))
    }
    fn empty(&self) {
        assert_eq!(fs::read_dir(&self.out).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&self.temp).unwrap().count(), 0);
    }
    fn no_backend(&self) {
        assert!(!self.model.with_extension("calls").exists());
    }
    fn spec(&self, value: Value) {
        fs::write(&self.model, serde_json::to_vec(&value).unwrap()).unwrap();
    }
}
fn response() -> Response {
    let username = "synthetic-peer";
    let voice = DatabaseVoice {
        silk: include_bytes!("../audio/silence.silk").to_vec(),
        evidence: VoiceEvidence {
            username: username.into(),
            message_source: "message/message_0.db".into(),
            message_table: format!("Msg_{:x}", md5::compute(username)),
            message_local_id: 7,
            server_id: 9007,
            create_time: 1700000000,
            media_source: "message/media_0.db".into(),
            media_rowid: 1,
            media_chat_name_id: 7,
            media_local_id: 42,
        },
    };
    let bytes = prepared_audio::encode(
        &voice,
        prepared_audio::Limits {
            max_audio_bytes: 16 * 1024 * 1024,
            max_response_bytes: 24 * 1024 * 1024,
        },
    )
    .unwrap();
    Response::ok(json!({"prepared_audio":serde_json::from_slice::<Value>(&bytes).unwrap()}))
}
fn frames(tool: &str, id: Value) -> String {
    [json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"audit","version":"1"},"capabilities":{}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":{"chat_name":"synthetic-peer","local_id":42}}})].iter().map(|v|format!("{v}\n")).collect()
}
fn process(
    f: &Fixture,
    tool: &str,
    id: Value,
    args: &[String],
    valid_config: bool,
) -> (Vec<Value>, String) {
    use std::os::windows::process::CommandExt;
    let payload = f.root.path().join("response.json");
    fs::write(&payload, serde_json::to_vec(&response()).unwrap()).unwrap();
    let home = f.root.path().join("isolated-home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_host-security-probe"));
    command
        .env_clear()
        .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
        .env("PATH", "")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &home)
        .env("APPDATA", &home)
        .env("WX_CLI_HOME", &home)
        .env("TEMP", &f.temp)
        .env("TMP", &f.temp)
        .env(
            "WX_CLI_CONFIG",
            if valid_config {
                f.rt.config_path.clone()
            } else {
                f.root.path().join("must-not-read-account")
            },
        )
        .env("AUDIT_RESPONSE", &payload)
        .args(args)
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(frames(tool, id).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let replies: Vec<Value> = stdout
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(replies.len(), 2, "{stdout}\n{stderr}");
    (replies, stderr)
}

#[test]
fn unauthorized_cloud_never_enters_account_path_backend_or_ipc() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    for mode in 0..5 {
        let f = Fixture::new();
        let mut args = vec![
            "--backend".into(),
            "explicit-open-ai".into(),
            "--openai-base-url".into(),
            format!("http://{}/v1", listener.local_addr().unwrap()),
            "--openai-model".into(),
            "synthetic".into(),
            "--api-key-file".into(),
            f.model.to_string_lossy().into(),
        ];
        match mode {
            1 => {
                args.extend([
                    "--allow-upload".into(),
                    "--whisper-model".into(),
                    f.model.to_string_lossy().into(),
                ]);
            }
            2 => {
                args[1] = "local".into();
            }
            3 => {
                args.extend([
                    "--allow-upload".into(),
                    "--timeout-seconds".into(),
                    "0".into(),
                ]);
            }
            4 => {
                args.extend(["--allow-upload".into(), "--language".into(), " ".into()]);
            }
            _ => {}
        }
        let (reply, events) = process(&f, "transcribe_voice", json!(2), &args, false);
        assert_eq!(reply[1]["result"]["isError"], true, "{reply:?}");
        assert!(!events.contains("AUDIT_EVENT:"), "mode={mode}: {events}");
        assert!(!events.contains("must-not-read-account"));
        f.empty();
        f.no_backend();
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn malformed_exit_flags_with_valid_audio_reject_before_wav_and_backend() {
    for flag in [
        json!("0"),
        Value::Null,
        json!(true),
        json!(false),
        json!(1),
        json!(-1),
        json!(0.5),
        json!(0.0),
        json!(u64::MAX),
        json!([]),
        json!({}),
    ] {
        for op in [Operation::Decode, Operation::Transcribe] {
            let f = Fixture::new();
            let mut r = response();
            r.data["exit_code"] = flag.clone();
            let result = f.finish(op, &f.args(), r);
            assert!(
                matches!(result, Err(DispatchError::QueryFailed)),
                "{flag}: {result:?}"
            );
            f.empty();
            f.no_backend();
        }
    }
    let f = Fixture::new();
    let mut r = response();
    r.data["exit_code"] = json!(0);
    assert!(f.finish(Operation::Transcribe, &f.args(), r).is_ok());
}

#[test]
fn corrupted_prepared_audio_and_media_id_mismatch_never_reach_backend_or_wav() {
    for mode in 0..17 {
        for op in [Operation::Decode, Operation::Transcribe] {
            let f = Fixture::new();
            let mut r = response();
            let p = &mut r.data["prepared_audio"];
            match mode {
                0 => p["version"] = json!(2),
                1 => p["silk_sha256"] = json!("0".repeat(64)),
                2 => p["silk_base64"] = json!("invalid"),
                3 => p["silk_size_bytes"] = json!(0),
                4 => p["evidence"]["media_local_id"] = json!(43),
                5 => p["evidence"]["message_source"] = json!("../other.db"),
                6 => p["evidence"]["message_table"] = json!("Msg_wrong"),
                7 => p["unknown"] = json!(true),
                8 => p["evidence"]["username"] = json!("a\nb"),
                9 => *p = Value::Null,
                10 => r.ok = false,
                11 => r.error = Some("SYNTHETIC_SECRET".into()),
                12 => r.data["error"] = json!(false),
                13 => p["silk_size_bytes"] = json!(16 * 1024 * 1024 + 1),
                14 => p["silk_size_bytes"] = json!(u64::MAX),
                15 => p["evidence"]["media_local_id"] = json!("42"),
                16 => p["silk_sha256"] = json!("X".repeat(64)),
                _ => unreachable!(),
            }
            assert!(f.finish(op, &f.args(), r).is_err(), "mode={mode}");
            f.empty();
            f.no_backend();
        }
    }
}

#[test]
fn bound_runtime_identity_cannot_be_switched_before_finish() {
    for mode in 0..9 {
        let f = Fixture::new();
        let ctx = CallContext::default();
        let pending = Args::default()
            .prepare(Operation::Decode, 42, Some(&f.out), &ctx)
            .unwrap()
            .bind(&f.rt)
            .unwrap();
        let mut other = f.rt.clone();
        match mode {
            0 => other.id.push('B'),
            1 => other.root.push("other"),
            2 => other.directory.push("other"),
            3 => other.config_path.push("other"),
            4 => other.config.db_dir.push("other"),
            5 => other.config.keys_file.push("other"),
            6 => other.config.decrypted_dir.push("other"),
            7 => other.config.wechat_process.push('B'),
            8 => other.config.key_store = Some(f.root.path().join("other-store.dpapi")),
            _ => unreachable!(),
        }
        assert!(matches!(
            pending.finish(response(), &other, &ctx, || Ok(())),
            Err(DispatchError::Unavailable)
        ));
        f.empty();
    }
}

#[test]
fn path_overlap_matrix_rejects_account_model_temp_and_cache_aliases() {
    for mode in 0..9 {
        let f = Fixture::new();
        let mut args = f.args();
        match mode {
            0 => args.backend.temp_root = Some(f.rt.config.db_dir.clone()),
            1 => args.backend.temp_root = Some(f.rt.config.decrypted_dir.clone()),
            2 => {
                fs::create_dir_all(f.rt.cache_dir()).unwrap();
                args.backend.temp_root = Some(f.rt.cache_dir());
            }
            3 => args.backend.temp_root = Some(f.model.parent().unwrap().to_owned()),
            4 => args.voice_cache_file = Some(f.temp.join("cache.json")),
            5 => args.voice_cache_file = Some(f.rt.config.db_dir.join("cache.json")),
            6 => args.voice_cache_file = Some(f.model.parent().unwrap().join("cache.json")),
            7 => {
                fs::hard_link(&f.model, &f.cache).unwrap();
                args.voice_cache_file = Some(f.cache.clone());
            }
            8 => {
                fs::hard_link(&f.rt.config.keys_file, &f.cache).unwrap();
                args.voice_cache_file = Some(f.cache.clone());
            }
            _ => unreachable!(),
        }
        let original_model = fs::read(&f.model).unwrap();
        let original_key = fs::read(&f.rt.config.keys_file).unwrap();
        assert!(
            f.finish(Operation::Transcribe, &args, response()).is_err(),
            "mode={mode}"
        );
        assert_eq!(fs::read(&f.model).unwrap(), original_model);
        assert_eq!(fs::read(&f.rt.config.keys_file).unwrap(), original_key);
        f.no_backend();
        f.empty();
    }
}

#[test]
fn default_temporary_roots_are_unique_and_removed_for_success_and_failure() {
    let f = Fixture::new();
    let mut roots = Vec::new();
    for mode in ["ok", "fail", "malformed", "ok"] {
        f.spec(json!({"mode":mode}));
        let mut args = f.args();
        args.backend.temp_root = None;
        let result = f.finish(Operation::Transcribe, &args, response());
        assert_eq!(result.is_ok(), mode == "ok", "{result:?}");
        let lines = fs::read_to_string(f.model.with_extension("calls")).unwrap();
        let path = PathBuf::from(lines.lines().last().unwrap());
        let request_root = path
            .ancestors()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("wx-cli-mcp-voice-"))
            })
            .unwrap()
            .to_owned();
        assert!(!path.exists());
        assert!(!request_root.exists());
        assert!(!roots.contains(&request_root));
        roots.push(request_root);
        f.empty();
    }
}

#[test]
fn backend_failure_malformed_result_and_timeout_do_not_report_success_or_leave_temps() {
    for mode in ["fail", "malformed", "sleep"] {
        let f = Fixture::new();
        f.spec(json!({"mode":mode}));
        let mut args = f.args();
        args.backend.timeout_seconds = 1;
        args.voice_cache_file = Some(f.cache.clone());
        let start = Instant::now();
        let result = f.finish(Operation::Transcribe, &args, response());
        assert!(result.is_err(), "{mode}: {result:?}");
        assert!(start.elapsed() < Duration::from_secs(3));
        f.empty();
        assert!(!f.cache.exists());
        assert!(!format!("{result:?}").contains("SYNTHETIC_BACKEND_SECRET"));
    }
}

#[test]
fn real_host_protocol_checks_escaped_transcript_and_long_request_id() {
    for (text, id) in [
        ("\"\\\n\t".repeat(180), json!(2)),
        ("small".repeat(40), json!("\"\\\n\t".repeat(105))),
    ] {
        let f = Fixture::new();
        f.spec(json!({"text":text}));
        let mut args = f.local_cli();
        args.extend(["--max-frame-bytes".into(), "1024".into()]);
        assert!(frames("transcribe_voice", id.clone())
            .lines()
            .all(|line| line.len() <= 1024));
        let (reply, events) = process(&f, "transcribe_voice", id, &args, true);
        assert_eq!(reply[1]["result"]["isError"], true, "{reply:?} {events}");
        assert!(events.contains("AUDIT_EVENT:ipc:"));
        assert!(events.contains("AUDIT_EVENT:backend-build"));
        f.empty();
    }
}

#[test]
fn real_host_decode_budget_checks_full_id_before_wav_commit() {
    let f = Fixture::new();
    let args = vec![
        "--media-output-root".into(),
        f.out.to_string_lossy().into(),
        "--max-frame-bytes".into(),
        "1024".into(),
    ];
    let (reply, events) = process(
        &f,
        "decode_voice",
        json!("\"\\\n\t".repeat(105)),
        &args,
        true,
    );
    assert_eq!(reply[1]["result"]["isError"], true, "{reply:?} {events}");
    assert!(events.contains("AUDIT_EVENT:ipc:"));
    f.empty();
    f.no_backend();
}

#[test]
fn explicit_cloud_loopback_only_success_and_backend_error_are_distinguished() {
    for status in [200, 401] {
        let f = Fixture::new();
        let key = f.root.path().join("credential.txt");
        fs::write(&key, b"SYNTHETIC_LOOPBACK_ONLY").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, peer)) => {
                        assert!(peer.ip().is_loopback());
                        break s;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "loopback request missing");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 8192];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
                assert!(bytes.len() < 1024 * 1024);
            }
            let request = String::from_utf8_lossy(&bytes);
            assert!(request.starts_with("POST /v1/audio/transcriptions "));
            assert!(request.contains("SYNTHETIC_LOOPBACK_ONLY"));
            assert!(bytes.windows(4).any(|s| s == b"RIFF"));
            let body = if status == 200 {
                r#"{"text":"loopback transcript","language":"zh"}"#
            } else {
                r#"{"error":"SYNTHETIC_REMOTE_SECRET"}"#
            };
            write!(stream,"HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let args = Args {
            backend: BackendArgs {
                backend: BackendKind::ExplicitOpenAi,
                allow_upload: true,
                openai_base_url: Some(format!("http://{address}/v1")),
                openai_model: Some("synthetic-model".into()),
                api_key_file: Some(key.clone()),
                timeout_seconds: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = f.finish(Operation::Transcribe, &args, response());
        server.join().unwrap();
        assert_eq!(result.is_ok(), status == 200, "{result:?}");
        assert!(!format!("{result:?}").contains("SYNTHETIC_REMOTE_SECRET"));
        assert_eq!(fs::read(&key).unwrap(), b"SYNTHETIC_LOOPBACK_ONLY");
        f.empty();
        f.no_backend();
    }
}

#[test]
fn junction_temp_output_and_cache_parent_are_rejected() {
    use std::os::windows::process::CommandExt;
    for mode in 0..3 {
        let f = Fixture::new();
        let link = f.root.path().join("alias");
        let output = Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&f.rt.config.db_dir)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut args = f.args();
        let ctx = CallContext::default();
        let result = match mode {
            0 => args
                .prepare(Operation::Decode, 42, Some(&link), &ctx)
                .and_then(|p| p.bind(&f.rt))
                .and_then(|p| p.finish(response(), &f.rt, &ctx, || Ok(()))),
            1 => {
                args.backend.temp_root = Some(link);
                f.finish(Operation::Transcribe, &args, response())
            }
            2 => {
                args.voice_cache_file = Some(link.join("cache.json"));
                f.finish(Operation::Transcribe, &args, response())
            }
            _ => unreachable!(),
        };
        assert!(result.is_err(), "junction mode={mode}");
        assert_eq!(fs::read_dir(&f.rt.config.db_dir).unwrap().count(), 0);
        f.empty();
        f.no_backend();
    }
}

#[test]
#[ignore = "Windows symbolic-link creation requires privilege; junctions tested separately"]
fn symbolic_link_model_and_cache_are_rejected() {
    for mode in 0..2 {
        let f = Fixture::new();
        let mut args = f.args();
        if mode == 0 {
            let link = f.root.path().join("model-link");
            std::os::windows::fs::symlink_file(&f.model, &link).unwrap();
            args.backend.whisper_model = Some(link);
        } else {
            std::os::windows::fs::symlink_file(&f.rt.config.keys_file, &f.cache).unwrap();
            args.voice_cache_file = Some(f.cache.clone());
        }
        assert!(f.finish(Operation::Transcribe, &args, response()).is_err());
        f.empty();
        f.no_backend();
    }
}

#[test]
fn response_budget_rejection_must_precede_transcript_cache_commit() {
    let f = Fixture::new();
    f.spec(json!({"text":"\"\\\n\t".repeat(180)}));
    let mut args = f.local_cli();
    args.extend([
        "--voice-cache-file".into(),
        f.cache.to_string_lossy().into(),
        "--max-frame-bytes".into(),
        "1024".into(),
    ]);
    let (reply, events) = process(&f, "transcribe_voice", json!(2), &args, true);
    assert_eq!(reply[1]["result"]["isError"], true, "{reply:?} {events}");
    assert!(events.contains("AUDIT_EVENT:backend-build"));
    f.empty();
    println!(
        "CACHE AFTER RESPONSE REJECTION exists={} bytes={}",
        f.cache.exists(),
        fs::metadata(&f.cache).map(|m| m.len()).unwrap_or(0)
    );
    assert!(
        !f.cache.exists(),
        "transcript was committed before complete response budget validation"
    );
}

#[test]
fn local_backend_uses_only_remaining_host_deadline_cached_and_uncached() {
    for cached in [false, true] {
        let f = Fixture::new();
        f.spec(json!({"mode":"sleep"}));
        let mut args = f.args();
        args.backend.timeout_seconds = 10;
        if cached {
            args.voice_cache_file = Some(f.cache.clone());
        }
        let ctx = CallContext::new(Default::default(), Duration::from_millis(2200));
        let pending = args
            .prepare(Operation::Transcribe, 42, None, &ctx)
            .unwrap()
            .bind(&f.rt)
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let start = Instant::now();
        let result = pending.finish(response(), &f.rt, &ctx, || Ok(()));
        assert!(
            matches!(result, Err(DispatchError::TimedOut)),
            "cached={cached} {result:?}"
        );
        assert!(start.elapsed() < Duration::from_millis(2800));
        assert!(f.model.with_extension("calls").exists());
        assert!(!f.cache.exists());
        f.empty();
    }
}

#[test]
fn final_commit_denial_must_precede_transcript_cache_commit() {
    let f = Fixture::new();
    let mut args = f.args();
    args.voice_cache_file = Some(f.cache.clone());
    let ctx = CallContext::default();
    let pending = args
        .prepare(Operation::Transcribe, 42, None, &ctx)
        .unwrap()
        .bind(&f.rt)
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let result = pending.finish(response(), &f.rt, &ctx, || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            Err(DispatchError::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(matches!(result, Err(DispatchError::Cancelled)));
    assert_eq!(calls.get(), 3);
    f.empty();
    println!(
        "CACHE AFTER FINAL COMMIT DENIAL exists={}",
        f.cache.exists()
    );
    assert!(
        !f.cache.exists(),
        "cache committed before final host authorization callback"
    );
}

#[test]
fn response_budget_rejection_keeps_existing_cache_byte_identical_on_hit_and_miss() {
    for hit in [true, false] {
        let f = Fixture::new();
        f.spec(json!({"text":"seed".repeat(50)}));
        let mut args = f.local_cli();
        args.extend([
            "--voice-cache-file".into(),
            f.cache.to_string_lossy().into(),
            "--max-frame-bytes".into(),
            "1024".into(),
        ]);
        let (reply, events) = process(&f, "transcribe_voice", json!(2), &args, true);
        assert_eq!(
            reply[1]["result"]["isError"], false,
            "seed: {reply:?} {events}"
        );
        let original = fs::read(&f.cache).unwrap();
        let id = if hit {
            json!("\"\\\n\t".repeat(105))
        } else {
            f.spec(json!({"text":"\"\\\n\t".repeat(180)}));
            json!(2)
        };
        let (reply, events) = process(&f, "transcribe_voice", id, &args, true);
        assert_eq!(
            reply[1]["result"]["isError"], true,
            "hit={hit} {reply:?} {events}"
        );
        assert_eq!(
            fs::read(&f.cache).unwrap(),
            original,
            "hit={hit}: existing cache changed after response rejection"
        );
        assert_eq!(
            fs::read_to_string(f.model.with_extension("calls"))
                .unwrap()
                .lines()
                .count(),
            if hit { 1 } else { 2 }
        );
        f.empty();
    }
}

#[test]
fn cancelled_new_result_keeps_existing_cache_byte_identical() {
    let f = Fixture::new();
    let mut args = f.args();
    args.voice_cache_file = Some(f.cache.clone());
    f.finish(Operation::Transcribe, &args, response()).unwrap();
    let original = fs::read(&f.cache).unwrap();
    f.spec(json!({"text":"new transcription must not be stored"}));
    let ctx = CallContext::default();
    let calls = std::cell::Cell::new(0);
    let pending = args
        .prepare(Operation::Transcribe, 42, None, &ctx)
        .unwrap()
        .bind(&f.rt)
        .unwrap();
    let result = pending.finish(response(), &f.rt, &ctx, || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            Err(DispatchError::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(
        matches!(result, Err(DispatchError::Cancelled)),
        "{result:?}"
    );
    assert_eq!(fs::read(&f.cache).unwrap(), original);
    f.empty();
}

#[test]
fn callback_stage_counts_distinguish_cache_miss_hit_and_uncached() {
    let f = Fixture::new();
    let mut args = f.args();
    args.voice_cache_file = Some(f.cache.clone());
    for (mode, expected) in [("miss", 5), ("hit", 3), ("uncached", 3)] {
        if mode == "uncached" {
            args.voice_cache_file = None;
        }
        let ctx = CallContext::default();
        let stages = std::cell::RefCell::new(Vec::new());
        let pending = args
            .prepare(Operation::Transcribe, 42, None, &ctx)
            .unwrap()
            .bind(&f.rt)
            .unwrap();
        pending
            .finish(response(), &f.rt, &ctx, || {
                stages.borrow_mut().push((
                    f.cache.exists(),
                    fs::read_dir(f.cache.parent().unwrap()).unwrap().count(),
                ));
                Ok(())
            })
            .unwrap();
        let stages = stages.into_inner();
        println!("CALLBACK STAGES {mode}: {stages:?}");
        assert_eq!(stages.len(), expected);
        if mode == "miss" {
            assert_eq!(stages[2], (false, 0));
            assert!(!stages[3].0);
            assert!(
                stages[3].1 >= 2,
                "persist callback must observe lock and staged cache"
            );
            assert_eq!(stages[4], (true, 1));
        }
        f.empty();
    }
}

#[test]
fn actual_cache_persist_rejection_preserves_reason_old_bytes_and_cleans_staging() {
    fn reason(mode: usize) -> DispatchError {
        match mode {
            0 => DispatchError::Cancelled,
            1 => DispatchError::TimedOut,
            2 => DispatchError::ResultLimit,
            _ => DispatchError::Unavailable,
        }
    }
    for existing in [false, true] {
        for mode in 0..4 {
            let f = Fixture::new();
            let mut args = f.args();
            args.voice_cache_file = Some(f.cache.clone());
            let original = if existing {
                f.finish(Operation::Transcribe, &args, response()).unwrap();
                let bytes = fs::read(&f.cache).unwrap();
                f.spec(json!({"text":"new distinct cache key"}));
                Some(bytes)
            } else {
                None
            };
            let ctx = CallContext::default();
            let calls = std::cell::Cell::new(0);
            let pending = args
                .prepare(Operation::Transcribe, 42, None, &ctx)
                .unwrap()
                .bind(&f.rt)
                .unwrap();
            let result = pending.finish(response(), &f.rt, &ctx, || {
                calls.set(calls.get() + 1);
                if calls.get() == 4 {
                    assert!(
                        fs::read_dir(f.cache.parent().unwrap()).unwrap().count()
                            >= if existing { 3 } else { 2 }
                    );
                    Err(reason(mode))
                } else {
                    Ok(())
                }
            });
            assert_eq!(calls.get(), 4);
            assert_eq!(
                format!("{:?}", result.unwrap_err()),
                format!("{:?}", reason(mode))
            );
            if let Some(bytes) = original {
                assert_eq!(fs::read(&f.cache).unwrap(), bytes);
            } else {
                assert!(!f.cache.exists());
            }
            assert_eq!(
                fs::read_dir(f.cache.parent().unwrap()).unwrap().count(),
                usize::from(existing)
            );
            f.empty();
        }
    }
}

#[test]
fn original_host_guard_is_rechecked_between_cache_preflight_and_persist() {
    for existing in [false, true] {
        let f = Fixture::new();
        let mut args = f.args();
        args.voice_cache_file = Some(f.cache.clone());
        let original = if existing {
            f.finish(Operation::Transcribe, &args, response()).unwrap();
            let bytes = fs::read(&f.cache).unwrap();
            f.spec(json!({"text":"new cache key"}));
            Some(bytes)
        } else {
            None
        };
        let ctx = CallContext::default();
        let calls = std::cell::Cell::new(0);
        let old_identity = same_file::Handle::from_path(&f.rt.config.db_dir).unwrap();
        let moved = f.root.path().join("original-account-db");
        let pending = args
            .prepare(Operation::Transcribe, 42, None, &ctx)
            .unwrap()
            .bind(&f.rt)
            .unwrap();
        let result = pending.finish(response(), &f.rt, &ctx, || {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                fs::rename(&f.rt.config.db_dir, &moved).unwrap();
                fs::create_dir(&f.rt.config.db_dir).unwrap();
                assert_eq!(same_file::Handle::from_path(&moved).unwrap(), old_identity);
                assert_ne!(
                    same_file::Handle::from_path(&f.rt.config.db_dir).unwrap(),
                    old_identity
                );
            }
            Ok(())
        });
        assert!(
            matches!(result, Err(DispatchError::Unavailable)),
            "{result:?}"
        );
        assert_eq!(
            calls.get(),
            3,
            "original guard must reject before invoking the fourth callback"
        );
        if let Some(bytes) = original {
            assert_eq!(fs::read(&f.cache).unwrap(), bytes);
        } else {
            assert!(!f.cache.exists());
        }
        assert_eq!(
            fs::read_dir(f.cache.parent().unwrap()).unwrap().count(),
            usize::from(existing)
        );
        f.empty();
    }
}
