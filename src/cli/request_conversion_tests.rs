use crate::service::operations::Operation;

fn parse<T: clap::Args + clap::FromArgMatches>(argv: &[&str]) -> T {
    let matches = T::augment_args(clap::Command::new("fixture"))
        .try_get_matches_from(argv)
        .unwrap();
    T::from_arg_matches(&matches).unwrap()
}

#[test]
fn backend_defaults_and_canonical_names_cross_the_cli_boundary() {
    use super::operation_args::asr::BackendArgs as CliArgs;
    use crate::service::operation_requests::asr::BackendArgs;
    let defaults: BackendArgs = parse::<CliArgs>(&["fixture"]).into();
    let wire = serde_json::to_value(&defaults).unwrap();
    assert_eq!(wire, serde_json::to_value(BackendArgs::default()).unwrap());
    assert_eq!(wire["backend"], "whisper_cpp");
    assert_eq!(wire["timeout_seconds"], 120);
    assert_eq!(wire["allow_upload"], false);
    for (name, expected) in [
        ("whisper_cpp", "whisper_cpp"),
        ("python_whisper", "python_whisper"),
        ("openai_compatible", "openai_compatible"),
    ] {
        let args: BackendArgs = parse::<CliArgs>(&["fixture", "--backend", name]).into();
        assert_eq!(serde_json::to_value(args).unwrap()["backend"], expected);
    }
    for old in ["local", "openai", "explicit-open-ai"] {
        assert!(
            <CliArgs as clap::Args>::augment_args(clap::Command::new("fixture"))
                .try_get_matches_from(["fixture", "--backend", old])
                .is_err()
        );
    }
}

#[test]
fn mcp_host_conversion_preserves_explicit_authorization_and_paths() {
    let parsed = parse::<super::operation_args::mcp_voice::Args>(&[
        "fixture",
        "--backend",
        "openai_compatible",
        "--allow-upload",
        "--openai-base-url",
        "https://example.invalid/v1",
        "--openai-model",
        "synthetic",
        "--api-key-file",
        "synthetic.key",
        "--voice-cache-file",
        "synthetic-cache.json",
        "--language",
        "zh",
        "--timeout-seconds",
        "37",
    ]);
    let voice: crate::service::mcp::VoiceSettings = parsed.into();
    let wire = serde_json::to_value(&voice).unwrap();
    assert_eq!(wire["backend"]["allow_upload"], true);
    assert_eq!(wire["backend"]["api_key_file"], "synthetic.key");
    assert_eq!(wire["voice_cache_file"], "synthetic-cache.json");
    assert_eq!(wire["backend"]["timeout_seconds"], 37);
    let host = crate::service::mcp::HostSettings {
        voice,
        ..Default::default()
    };
    let wire = serde_json::to_value(&host).unwrap();
    let decoded: crate::service::mcp::HostSettings = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
    let mut extra = wire;
    extra["tool_supplied_authorization"] = serde_json::json!(true);
    assert!(serde_json::from_value::<crate::service::mcp::HostSettings>(extra).is_err());
}

#[test]
fn deserialized_requests_cannot_bypass_clap_limits_or_conflicts() {
    fn args<T: clap::Args + clap::FromArgMatches>(argv: &[&str]) -> T {
        let matches = T::augment_args(clap::Command::new("fixture"))
            .try_get_matches_from(argv)
            .unwrap();
        T::from_arg_matches(&matches).unwrap()
    }
    fn rejects(operation: &Operation, pointer: &str, value: serde_json::Value) {
        let mut wire = serde_json::to_value(operation).unwrap();
        *wire.pointer_mut(pointer).unwrap() = value;
        let decoded: Operation = serde_json::from_value(wire).unwrap();
        assert!(
            decoded.validate_request().is_err(),
            "accepted invalid {pointer}"
        );
    }
    let image = Operation::ImageKeys {
        args: args::<super::operation_args::image_keys::Args>(&["fixture", "--offline"]).into(),
    };
    assert!(image.validate_request().is_ok());
    for (pointer, value) in [
        ("/args/args/timeout", serde_json::json!(0)),
        ("/args/args/timeout", serde_json::json!(3601)),
        ("/args/args/max_mib", serde_json::json!(0)),
        ("/args/args/max_mib", serde_json::json!(32769)),
        ("/args/args/authorize_memory_scan", serde_json::json!(true)),
        ("/args/args/offline", serde_json::json!(false)),
    ] {
        rejects(&image, pointer, value);
    }
    let plan = Operation::ChatPlan {
        args: args::<super::operation_args::chat_plan::Args>(&[
            "fixture",
            "--decrypted-dir",
            "must-not-read",
            "--user",
            "fixture",
            "--output",
            "must-not-create.csv",
        ])
        .into(),
    };
    assert!(plan.validate_request().is_ok());
    rejects(&plan, "/args/args/threads", serde_json::json!(0));
    rejects(&plan, "/args/args/threads", serde_json::json!(7));
    rejects(&plan, "/args/args/users", serde_json::json!([]));
    let setup = Operation::Setup {
        args: args::<super::operation_args::setup_native::Args>(&["fixture"]).into(),
    };
    assert!(setup.validate_request().is_ok());
    rejects(&setup, "/args/args/yes", serde_json::json!(true));
    rejects(&setup, "/args/args/apply", serde_json::json!(true));
    rejects(&setup, "/args/args/interactive", serde_json::json!(true));
    rejects(
        &setup,
        "/args/args/openai_key_env",
        serde_json::json!("INVALID=NAME"),
    );
}
