use mcp_voice_host::{
    asr::{BackendArgs, BackendKind},
    config::Config,
    ipc::Response,
    mcp_voice::{Args, Operation},
    protocol::{CallContext, DispatchError},
    runtime::RuntimeContext,
    toolkit_asr::{
        database_media::{DatabaseVoice, VoiceEvidence},
        prepared_audio,
    },
};
use serde_json::json;
use std::{fs, path::Path};

fn runtime(root: &Path) -> RuntimeContext {
    let account = root.join("account");
    fs::create_dir(&account).unwrap();
    RuntimeContext {
        config: Config {
            db_dir: account.join("db"),
            keys_file: account.join("keys.json"),
            decrypted_dir: account.join("decrypted"),
            wechat_process: "Weixin.exe".into(),
        },
        config_path: account.join("config.json"),
        root: account.clone(),
        id: "test-account".into(),
        directory: account.join("runtime"),
    }
}

fn response(id: i64) -> Response {
    let username = "voice-test";
    let voice = DatabaseVoice {
        silk: include_bytes!("../../audio/silence.silk").to_vec(),
        evidence: VoiceEvidence {
            username: username.into(),
            message_source: "message/message_0.db".into(),
            message_table: format!("Msg_{:x}", md5::compute(username)),
            message_local_id: 7,
            server_id: 22,
            create_time: 1700000000,
            media_source: "message/media_0.db".into(),
            media_rowid: 3,
            media_chat_name_id: 9,
            media_local_id: id,
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
    Response::ok(
        json!({"prepared_audio": serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()}),
    )
}

#[test]
fn real_decode_and_noclobber() {
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let args = Args::default();
    let context = CallContext::default();
    let result = args
        .prepare(Operation::Decode, 42, Some(&output), &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &rt, &context, || Ok(()))
        .unwrap();
    let text = result.data["mcp_text"].as_str().unwrap();
    assert!(text.starts_with("解码成功!\n  文件: "));
    assert!(!text.contains("prepared_audio"));
    let files: Vec<_> = fs::read_dir(&output)
        .unwrap()
        .map(|x| x.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let wav = fs::read(&files[0]).unwrap();
    assert_eq!(
        wav,
        mcp_voice_host::toolkit_asr::prepare_wav_bytes(include_bytes!("../../audio/silence.silk"))
            .unwrap()
    );
    let retry = args
        .prepare(Operation::Decode, 42, Some(&output), &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &rt, &context, || Ok(()));
    assert!(matches!(retry, Err(DispatchError::QueryFailed)));
    assert_eq!(fs::read(&files[0]).unwrap(), wav);
}

#[test]
fn invalid_identity_and_commit_denial_leave_no_wav() {
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let context = CallContext::default();
    for wrong_id in [true, false] {
        let result = Args::default()
            .prepare(Operation::Decode, 42, Some(&output), &context)
            .unwrap()
            .bind(&rt)
            .unwrap()
            .finish(
                response(if wrong_id { 7 } else { 42 }),
                &rt,
                &context,
                || Err(DispatchError::Cancelled),
            );
        assert!(matches!(
            result,
            Err(DispatchError::InvalidResponse | DispatchError::Cancelled)
        ));
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
    }
    // 第二次回调发生于 WAV 暂存完成后，覆盖真正的提交前拒绝路径。
    let calls = std::cell::Cell::new(0);
    let result = Args::default()
        .prepare(Operation::Decode, 42, Some(&output), &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &rt, &context, || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                Err(DispatchError::TimedOut)
            } else {
                Ok(())
            }
        });
    assert!(matches!(result, Err(DispatchError::TimedOut)));
    assert_eq!(calls.get(), 2);
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
}

#[test]
fn authorization_and_binding_fail_closed() {
    let context = CallContext::default();
    let args = Args {
        backend: BackendArgs {
            backend: BackendKind::ExplicitOpenAi,
            api_key_file: Some("missing-secret".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        args.prepare(Operation::Transcribe, 42, None, &context),
        Err(DispatchError::Unavailable)
    ));
    assert!(matches!(
        Args::default().prepare(Operation::Decode, 42, None, &context),
        Err(DispatchError::Unavailable)
    ));
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let mut other = rt.clone();
    other.id = "other".into();
    let result = Args::default()
        .prepare(Operation::Decode, 42, Some(&output), &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &other, &context, || Ok(()));
    assert!(matches!(result, Err(DispatchError::Unavailable)));
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
}

#[test]
fn local_process_cache_hit_and_ephemeral_wav() {
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let temp = root.path().join("temporary");
    let cache_dir = root.path().join("transcripts");
    let backend_dir = root.path().join("backend");
    for path in [&temp, &cache_dir, &backend_dir] {
        fs::create_dir(path).unwrap();
    }
    let model = backend_dir.join("model.bin");
    fs::write(&model, b"synthetic model").unwrap();
    let args = Args {
        backend: BackendArgs {
            whisper_binary: Some(env!("CARGO_BIN_EXE_voice-probe").into()),
            whisper_model: Some(model.clone()),
            temp_root: Some(temp.clone()),
            ..Default::default()
        },
        voice_cache_file: Some(cache_dir.join("voices.json")),
    };
    let context = CallContext::default();
    for _ in 0..2 {
        let result = args
            .prepare(Operation::Transcribe, 42, None, &context)
            .unwrap()
            .bind(&rt)
            .unwrap()
            .finish(response(42), &rt, &context, || Ok(()))
            .unwrap();
        assert!(result.data["mcp_text"]
            .as_str()
            .unwrap()
            .ends_with("] (zh)\nsynthetic transcript"));
        assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
    }
    assert_eq!(
        fs::read(model.with_extension("calls")).unwrap(),
        b"called\n"
    );
    let uncached = Args {
        voice_cache_file: None,
        ..args
    };
    uncached
        .prepare(Operation::Transcribe, 42, None, &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &rt, &context, || Ok(()))
        .unwrap();
    assert_eq!(
        fs::read(model.with_extension("calls")).unwrap(),
        b"called\ncalled\n"
    );
    assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
}

#[test]
fn backend_error_payloads_are_query_failures() {
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let context = CallContext::default();
    for data in [
        json!({"exit_code": 1}),
        json!({"exit_code": -1}),
        json!({"error": "PRIVATE_BACKEND_ERROR"}),
    ] {
        let result = Args::default()
            .prepare(Operation::Decode, 42, Some(&output), &context)
            .unwrap()
            .bind(&rt)
            .unwrap()
            .finish(Response::ok(data), &rt, &context, || Ok(()));
        assert!(matches!(result, Err(DispatchError::QueryFailed)));
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
    }
}

#[test]
fn default_temp_is_request_scoped_and_cache_stays_reusable() {
    let root = tempfile::tempdir().unwrap();
    let rt = runtime(root.path());
    let backend_dir = root.path().join("backend");
    let cache_dir = root.path().join("cache");
    fs::create_dir(&backend_dir).unwrap();
    fs::create_dir(&cache_dir).unwrap();
    let model = backend_dir.join("model.bin");
    fs::write(&model, b"synthetic model").unwrap();
    let args = Args {
        backend: BackendArgs {
            whisper_binary: Some(env!("CARGO_BIN_EXE_voice-probe").into()),
            whisper_model: Some(model.clone()),
            ..Default::default()
        },
        voice_cache_file: Some(cache_dir.join("voices.json")),
    };
    let context = CallContext::default();
    for _ in 0..2 {
        args.prepare(Operation::Transcribe, 42, None, &context)
            .unwrap()
            .bind(&rt)
            .unwrap()
            .finish(response(42), &rt, &context, || Ok(()))
            .unwrap();
        let wav =
            std::path::PathBuf::from(fs::read_to_string(model.with_extension("wav-path")).unwrap());
        let isolated = wav
            .ancestors()
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("wx-cli-mcp-voice-"))
            })
            .unwrap();
        assert!(
            !isolated.exists(),
            "request temporary directory must be removed"
        );
    }
    assert_eq!(
        fs::read(model.with_extension("calls")).unwrap(),
        b"called\n"
    );
    assert!(args.backend.temp_root.is_none());
}

#[test]
fn relative_output_is_supported_but_parent_traversal_is_rejected() {
    let cwd = std::env::current_dir().unwrap();
    let root = tempfile::tempdir_in(&cwd).unwrap();
    let rt = runtime(root.path());
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let relative = output.strip_prefix(&cwd).unwrap();
    let context = CallContext::default();
    Args::default()
        .prepare(Operation::Decode, 42, Some(relative), &context)
        .unwrap()
        .bind(&rt)
        .unwrap()
        .finish(response(42), &rt, &context, || Ok(()))
        .unwrap();
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
    let traversal = relative.join("..").join("output");
    assert!(matches!(
        Args::default().prepare(Operation::Decode, 42, Some(&traversal), &context),
        Err(DispatchError::Unavailable)
    ));
}

#[test]
fn backend_uses_budget_remaining_after_ipc() {
    use std::time::{Duration, Instant};
    for cached in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let rt = runtime(root.path());
        let backend_dir = root.path().join("backend");
        let cache_dir = root.path().join("cache");
        fs::create_dir(&backend_dir).unwrap();
        fs::create_dir(&cache_dir).unwrap();
        let model = backend_dir.join("model.bin");
        fs::write(&model, b"sleep").unwrap();
        let args = Args {
            backend: BackendArgs {
                whisper_binary: Some(env!("CARGO_BIN_EXE_voice-probe").into()),
                whisper_model: Some(model.clone()),
                timeout_seconds: 10,
                ..Default::default()
            },
            voice_cache_file: cached.then(|| cache_dir.join("voices.json")),
        };
        let context = CallContext::new(Default::default(), Duration::from_secs(1));
        let pending = args
            .prepare(Operation::Transcribe, 42, None, &context)
            .unwrap()
            .bind(&rt)
            .unwrap();
        // 模拟 prepare 与 finish 之间的 IPC 等待；后端只能使用剩余的约 400ms。
        std::thread::sleep(Duration::from_millis(600));
        let start = Instant::now();
        let result = pending.finish(response(42), &rt, &context, || Ok(()));
        assert!(matches!(result, Err(DispatchError::TimedOut)), "{result:?}");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "backend reused the original timeout"
        );
        assert_eq!(
            fs::read(model.with_extension("calls")).unwrap(),
            b"called\n"
        );
        assert!(!model.with_extension("wav-path").exists());
        assert_eq!(fs::read_dir(&cache_dir).unwrap().count(), 0);
    }
}
