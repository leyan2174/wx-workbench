use crate::service::operations::Operation;

#[test]
fn removed_audio_commands_and_settings_are_rejected() {
    use clap::Parser;
    for argv in [
        vec!["wx", "audio", "convert", "input.silk"],
        vec!["wx", "audio", "transcribe", "input.silk"],
        vec!["wx", "chats", "transcribe"],
        vec!["wx", "chats", "transcribe-manifest"],
        vec!["wx", "chats", "export-all", "--with-transcriptions"],
        vec!["wx", "setup", "--backend", "python_whisper"],
        vec!["wx", "mcp", "--configured-local-python"],
        vec!["wx", "mcp", "--voice-cache-file", "cache.json"],
        vec!["wx", "tasks", "submit", "voice_mp3"],
        vec!["wx", "tasks", "submit", "export_all", "--include-voice"],
    ] {
        assert!(super::Cli::try_parse_from(argv).is_err());
    }
    for kind in [
        "transcribe_audio",
        "transcribe_chat",
        "transcribe_batch",
        "transcribe_database",
        "export_audio",
        "convert_audio",
    ] {
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({"kind":kind,"args":{}}))
                .is_err()
        );
    }
    let mut host = serde_json::to_value(crate::service::mcp::HostSettings::default()).unwrap();
    host["voice"] = serde_json::json!({});
    assert!(serde_json::from_value::<crate::service::mcp::HostSettings>(host).is_err());
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
    let mut wire = serde_json::to_value(&setup).unwrap();
    wire["args"]["args"]["openai_key_env"] = serde_json::json!("UNSUPPORTED");
    assert!(serde_json::from_value::<Operation>(wire).is_err());
}
