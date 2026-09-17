use super::*;
use crate::{ipc::Response, mcp::protocol::Protocol};

fn all_permissions() -> Args {
    Args {
        tasks: true,
        task_kind: plan::capabilities()
            .into_iter()
            .map(|entry| serde_json::from_value(entry["kind"].clone()).unwrap())
            .collect(),
        task_allow_media_write: true,
        task_allow_memory_scan: true,
        task_allow_media_download: true,
        ..Default::default()
    }
}

fn submit(kind: Kind, options: Options) -> Call {
    Call::Submit {
        idempotency_key: "12".repeat(32),
        task: Submission { kind, options },
    }
}

#[test]
fn task_discovery_is_opt_in_and_uses_shared_capabilities() {
    assert!(Args::default().tools().is_empty());
    let mut args = Args {
        tasks: true,
        ..Default::default()
    };
    assert_eq!(args.tools().len(), 4);
    args.task_kind = all_permissions().task_kind;
    let tools = args.tools();
    let submit = tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap();
    assert_eq!(
        submit.input_schema["properties"]["kind"]["enum"],
        json!(["wechat_decrypt"])
    );
    assert_eq!(
        all_permissions()
            .tools()
            .iter()
            .find(|tool| tool.name == "submit_task")
            .unwrap()
            .input_schema["properties"]["kind"]["enum"]
            .as_array()
            .unwrap()
            .len(),
        plan::capabilities().len()
    );
}

#[test]
fn model_flags_cannot_replace_host_authorization() {
    let complete = all_permissions();
    for (kind, options) in [
        (
            Kind::WechatKeys,
            Options {
                authorize_memory_scan: true,
                ..Default::default()
            },
        ),
        (
            Kind::ExportAll,
            Options {
                ..Default::default()
            },
        ),
        (
            Kind::SnsDecrypt,
            Options {
                include_sns_media: true,
                ..Default::default()
            },
        ),
    ] {
        let call = submit(kind, options);
        assert!(complete.authorize(&call));
        let mut limited = complete.clone();
        match kind {
            Kind::WechatKeys => limited.task_allow_memory_scan = false,
            Kind::ExportAll => limited.task_allow_media_write = false,
            _ => limited.task_allow_media_download = false,
        }
        assert!(!limited.authorize(&call));
        assert!(!Args::default().authorize(&call));
    }
    let mut limited = complete.clone();
    limited.task_allow_media_write = false;
    for kind in [Kind::ExportAll, Kind::DecodeImages, Kind::SnsDecrypt] {
        assert!(!limited.authorize(&submit(kind, Options::default())));
    }
    limited = complete;
    limited
        .task_kind
        .retain(|kind| !matches!(kind, Kind::SnsDecrypt));
    assert!(!limited.authorize(&submit(
        Kind::ExportAll,
        Options {
            include_sns: true,
            ..Default::default()
        }
    )));
}

#[test]
fn task_arguments_reject_paths_commands_and_invalid_bounds() {
    let id = "12".repeat(32);
    for arguments in [
        json!({"kind":"shell", "idempotency_key":id}),
        json!({"kind":"wechat_decrypt", "idempotency_key":"short"}),
        json!({"kind":"wechat_decrypt", "idempotency_key":id, "command":"cmd.exe"}),
        json!({"kind":"export_all", "idempotency_key":id, "options":{"output_root":"C:/other"}}),
        json!({"kind":"export_all", "idempotency_key":id, "options":{"allow_upload":"true"}}),
        json!(null),
    ] {
        assert!(matches!(
            parse("submit_task", &arguments),
            Err(DispatchError::InvalidArguments)
        ));
    }
    for arguments in [
        json!({"limit":0}),
        json!({"limit":129}),
        json!({"wait_ms":2001}),
        json!({"after":-1}),
        json!({"runtime_id":"other"}),
    ] {
        assert!(parse("get_task_events", &arguments).is_err());
    }
    assert!(parse("list_tasks", &json!({"config":"other"})).is_err());
    assert!(parse("get_task", &json!({"id":"AB".repeat(32)})).is_err());
    assert!(parse("cancel_task", &json!({"id":id})).is_ok());
    assert!(matches!(
        parse("get_task_events", &json!({})).unwrap(),
        Call::Events {
            after: 0,
            limit: 32,
            wait_ms: 0
        }
    ));
}

#[test]
fn service_errors_keep_codes_but_never_backend_text() {
    for code in [
        "submission_conflict",
        "settings_conflict",
        "configuration_changed",
        "not_found",
        "queue_full",
        "invalid_task",
        "outcome_unknown",
    ] {
        let result = failure(code);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], code);
    }
    assert!(!failure("SECRET_PATH_OR_KEY")
        .to_string()
        .contains("SECRET_PATH_OR_KEY"));
}

#[test]
fn protocol_lists_task_annotations_and_rejects_invalid_arguments_before_account_access() {
    let io = tokio::runtime::Runtime::new().unwrap();
    let account = Account::new(true);
    let invalidated = Cell::new(false);
    let args = all_permissions();
    let adapter = Adapter {
        query: |_request| Ok(Response::ok(json!({}))),
        account: &account,
        invalidated: &invalidated,
        io: &io,
        args: &args,
    };
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"submit_task","arguments":{"command":"anything"}}}),
    ];
    let input = messages
        .iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    Protocol::new(adapter)
        .serve(std::io::Cursor::new(input), &mut output, 1024 * 1024)
        .unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), crate::mcp::protocol::tools().len() + 5);
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "get_task")
            .unwrap()["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "cancel_task")
            .unwrap()["annotations"]["destructiveHint"],
        true
    );
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "submit_task")
            .unwrap()["annotations"]["openWorldHint"],
        true
    );
    assert_eq!(replies[2]["error"]["code"], -32602);
    assert!(account.runtime().is_none());
}

#[test]
fn ended_tool_context_never_means_background_cancellation() {
    use crate::mcp::protocol::CancellationToken;
    let io = tokio::runtime::Runtime::new().unwrap();
    let account = Account::new(true);
    let invalidated = Cell::new(false);
    let args = all_permissions();
    let mut protocol = Protocol::new(Adapter {
        query: |_request| Ok(Response::ok(json!({}))),
        account: &account,
        invalidated: &invalidated,
        io: &io,
        args: &args,
    });
    protocol.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}).to_string().as_bytes());
    protocol.handle(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    for cancelled in [true, false] {
        let token = CancellationToken::default();
        if cancelled {
            token.cancel();
        }
        let context = CallContext::new(token, std::time::Duration::ZERO);
        let reply = protocol.handle_with_context(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"submit_task","arguments":{"idempotency_key":"ab".repeat(32),"kind":"wechat_decrypt"}}}).to_string().as_bytes(),
            &context,
        ).unwrap();
        assert_eq!(reply["result"]["isError"], true);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not cancelled"), "{text}");
        assert!(text.contains("same key"), "{text}");
        assert!(account.runtime().is_none());
    }
}
