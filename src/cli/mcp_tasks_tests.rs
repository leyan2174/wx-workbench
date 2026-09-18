use super::*;
use crate::{ipc::Response, mcp::protocol::Protocol};

fn all_permissions() -> Args {
    Args {
        tasks: true,
        task_allow_artifact_read: true,
        task_allow_plan_scan: true,
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
        task: Submission { kind, options }.into(),
    }
}

fn plan_reference() -> Value {
    json!({"task_id":"a".repeat(64),"artifact_id":"b".repeat(64),"sha256":"c".repeat(64)})
}

fn plan_input(kind: &str, request: Value) -> Value {
    json!({"idempotency_key":"d".repeat(64),"kind":kind,"options":{(kind):request}})
}

#[test]
fn plan_host_scan_and_reference_read_permissions_are_independent() {
    use clap::Parser;
    #[derive(Parser)]
    struct Host {
        #[command(flatten)]
        args: Args,
    }
    assert!(Host::try_parse_from(["mcp", "--task-allow-plan-scan"]).is_err());
    let mut args = Host::try_parse_from([
        "mcp",
        "--tasks",
        "--task-kind",
        "chat_plan,chat_plan_review,chat_plan_apply",
    ])
    .unwrap()
    .args;
    let estimate = parse("submit_task", &plan_input("chat_plan", json!({}))).unwrap();
    let scan = parse(
        "submit_task",
        &plan_input("chat_plan", json!({"size_mode":"scan"})),
    )
    .unwrap();
    let review = parse(
        "submit_task",
        &plan_input(
            "chat_plan_review",
            json!({"plan_ref":plan_reference(),"changes":[]}),
        ),
    )
    .unwrap();
    let apply = parse(
        "submit_task",
        &plan_input("chat_plan_apply", json!({"plan_ref":plan_reference()})),
    )
    .unwrap();
    let read = parse("read_chat_plan", &json!({"plan_ref":plan_reference()})).unwrap();
    assert!(args.authorize(&estimate));
    for call in [&scan, &review, &apply, &read] {
        assert!(!args.authorize(call));
    }
    args.task_allow_memory_scan = true;
    args.task_allow_media_write = true;
    assert!(!args.authorize(&scan));
    let tools = args.tools();
    let schema = &tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap()
        .input_schema;
    assert_eq!(schema["properties"]["kind"]["enum"], json!(["chat_plan"]));
    assert_eq!(
        schema["properties"]["options"]["properties"]["chat_plan"]["properties"]["size_mode"]
            ["enum"],
        json!(["estimate"])
    );
    args.task_allow_plan_scan = true;
    assert!(args.authorize(&scan));
    assert!(!args.authorize(&review));
    args.task_allow_artifact_read = true;
    args.task_allow_plan_scan = false;
    for call in [&review, &apply, &read] {
        assert!(args.authorize(call));
    }
    for call in [
        Call::List {},
        Call::Get { id: "a".repeat(64) },
        Call::Cancel { id: "a".repeat(64) },
    ] {
        assert!(args.authorize(&call));
    }
    args.task_kind.clear();
    assert!(args.authorize(&read));
    assert!(!args.authorize(&apply));
    assert!(args
        .tools()
        .iter()
        .any(|tool| tool.name == "read_chat_plan"));
}

#[test]
fn plan_schemas_use_real_types_and_parsers_preserve_empty_selection() {
    let tools = all_permissions().tools();
    let properties = &tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap()
        .input_schema["properties"]["options"]["properties"];
    for name in ["chat_plan", "chat_plan_review", "chat_plan_apply"] {
        assert_eq!(properties[name]["type"], "object");
        assert_eq!(properties[name]["additionalProperties"], false);
    }
    assert_eq!(
        properties["chat_plan"]["properties"]["threads"]["maximum"],
        6
    );
    assert_eq!(
        properties["chat_plan"]["properties"]["size_mode"]["enum"],
        json!(["estimate", "scan"])
    );
    assert_eq!(
        properties["chat_plan_review"]["properties"]["changes"]["items"]["properties"]["export"]
            ["enum"],
        json!(["", "0", "1"])
    );
    assert_eq!(
        properties["chat_plan_apply"]["properties"]["plan_mode"]["default"],
        "blacklist"
    );
    for users in [None, Some(json!([])), Some(json!(["alice,bob"]))] {
        let mut request = json!({"threads":6,"start":"-1","end":"0"});
        if let Some(users) = &users {
            request["users"] = users.clone();
        }
        let Call::Submit { task, .. } =
            parse("submit_task", &plan_input("chat_plan", request)).unwrap()
        else {
            panic!()
        };
        let actual = task.options.chat_plan.unwrap();
        assert_eq!(
            serde_json::to_value(actual.users).unwrap(),
            users.unwrap_or(Value::Null)
        );
    }
    assert!(matches!(
        parse("read_chat_plan", &json!({"plan_ref":plan_reference()})).unwrap(),
        Call::ReadChatPlan {
            plan_mode: chat_plan::Mode::Blacklist,
            offset: 0,
            limit: 50,
            ..
        }
    ));
    for request in [
        json!({"threads":0}),
        json!({"threads":7}),
        json!({"threads":"1"}),
        json!({"users":"alice"}),
        json!({"size_mode":"memory"}),
        json!({"start":"2","end":"1"}),
        json!({"source_dir":"C:/private"}),
        json!({"allow_plan_scan":true}),
        json!({"decrypted_dir":"C:/private"}),
    ] {
        assert!(parse("submit_task", &plan_input("chat_plan", request)).is_err());
    }
    for changes in [
        json!([{"username":"a","export":true}]),
        json!([{"username":"a","export":"2"}]),
        json!([{"username":"a","export":"0","message_count":0}]),
        json!([{"username":"a","export":"0"},{"username":"a","export":"1"}]),
    ] {
        assert!(parse(
            "submit_task",
            &plan_input(
                "chat_plan_review",
                json!({"plan_ref":plan_reference(),"changes":changes})
            )
        )
        .is_err());
    }
    for kind in ["chat_plan", "chat_plan_review", "chat_plan_apply"] {
        let request = match kind {
            "chat_plan" => json!({}),
            "chat_plan_review" => json!({"plan_ref":plan_reference(),"changes":[]}),
            _ => json!({"plan_ref":plan_reference(),"dry_run":true}),
        };
        let mut wrong = plan_input(kind, request);
        wrong["kind"] = json!("export_all");
        assert!(parse("submit_task", &wrong).is_err());
        assert!(parse(
            "submit_task",
            &json!({"idempotency_key":"a".repeat(64),"kind":kind})
        )
        .is_err());
    }
    for (key, value) in [
        ("limit", json!(0)),
        ("limit", json!(101)),
        ("offset", json!(-1)),
        ("plan_mode", json!("all")),
        ("path", json!("C:/plan.csv")),
        ("task_allow_plan_scan", json!(true)),
    ] {
        let mut input = json!({"plan_ref":plan_reference()});
        input[key] = value;
        assert!(parse("read_chat_plan", &input).is_err());
    }
    let mut bad_ref = plan_reference();
    bad_ref["sha256"] = json!("C:/plan.csv");
    assert!(parse("read_chat_plan", &json!({"plan_ref":bad_ref})).is_err());
}

#[test]
fn denied_scan_submit_never_accesses_account_even_for_same_key() {
    let args = Args {
        tasks: true,
        task_kind: vec![Kind::ChatPlan],
        ..Default::default()
    };
    let account = Account::new(true);
    let invalidated = Cell::new(false);
    let io = tokio::runtime::Runtime::new().unwrap();
    let mut adapter = Adapter {
        query: |_request| Ok(Response::ok(json!({}))),
        account: &account,
        invalidated: &invalidated,
        io: &io,
        args: &args,
    };
    let input = plan_input("chat_plan", json!({"size_mode":"scan"}));
    for _ in 0..2 {
        let result = adapter
            .dispatch_task("submit_task", &input, &CallContext::default())
            .unwrap();
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "host_forbidden"
        );
    }
    assert!(account.runtime().is_none());
    assert!(!invalidated.get());
}

fn plan_page(rows: Vec<Value>, offset: u64, total: u64) -> Value {
    let next = offset + rows.len() as u64;
    json!({"version":1,"plan_ref":plan_reference(),"plan_mode":"blacklist","rows":rows,
        "offset":offset,"next_offset":if next<total {Some(next)} else {None},"total":total,
        "selected_count":total,"start_ts":null,"end_ts":null,"source_kind":"runtime_snapshot"})
}

#[test]
fn plan_pages_fit_full_frames_without_truncating_rows_or_identity() {
    let rows:Vec<_>=(0..5).map(|index|json!({"export":"","index":index+31,"username":format!("user-{index}"),
        "chat_name":"\\\"中文".repeat(40),"chat_type":"private","message_count":2,"first_time":"","last_time":"",
        "attachment_estimated_bytes":0,"attachment_scanned_bytes":null,"total_estimated_bytes":0,"size_status":"estimated","selected":true})).collect();
    let response_id = json!("\\\"".repeat(20));
    let first = content(plan_page(rows[..1].to_vec(), 7, 12), false);
    let frame = frame_size(&first, &response_id).unwrap() + 1;
    let context = artifact_context(frame, response_id.clone());
    let result = bounded_plan_content(plan_page(rows.clone(), 7, 12), &context).unwrap();
    assert!(frame_size(&result, &response_id).unwrap() <= frame);
    assert_eq!(result["structuredContent"]["rows"], json!([rows[0]]));
    assert_eq!(result["structuredContent"]["next_offset"], 8);
    assert_eq!(result["structuredContent"]["total"], 12);
    assert_eq!(result["structuredContent"]["selected_count"], 12);
    assert_eq!(result["structuredContent"]["plan_ref"], plan_reference());
    let final_page = plan_page(vec![rows[4].clone()], 11, 12);
    let final_context = artifact_context(frame + 128, response_id.clone());
    assert_eq!(
        bounded_plan_content(final_page.clone(), &final_context).unwrap()["structuredContent"],
        final_page
    );
    let too_small = artifact_context(frame_size(&first, &response_id).unwrap() - 1, response_id);
    assert!(matches!(
        bounded_plan_content(plan_page(vec![rows[0].clone()], 7, 12), &too_small),
        Err(DispatchError::ResultLimit)
    ));
    let empty = plan_page(vec![], 12, 12);
    assert_eq!(
        bounded_plan_content(empty.clone(), &context).unwrap()["structuredContent"],
        empty
    );
}

fn history_input(request: Value) -> Value {
    json!({"idempotency_key":"a".repeat(64),"kind":"export_history",
        "options":{"history_export":request}})
}

#[test]
fn history_schema_and_authorization_are_independent_of_media_and_artifact_read() {
    let args = Args {
        tasks: true,
        task_kind: vec![Kind::ExportHistory],
        ..Default::default()
    };
    let tools = args.tools();
    assert_eq!(tools.len(), 5);
    assert!(tools
        .iter()
        .all(|tool| !matches!(tool.name, "list_task_artifacts" | "read_task_artifact")));
    let schema = &tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap()
        .input_schema;
    assert_eq!(
        schema["properties"]["kind"]["enum"],
        json!(["export_history"])
    );
    let history = &schema["properties"]["options"]["properties"]["history_export"];
    assert_eq!(history["type"], "object");
    assert_eq!(history["additionalProperties"], false);
    assert_eq!(history["required"], json!(["chat"]));
    for name in ["chat", "since", "until", "format"] {
        assert_eq!(history["properties"][name]["type"], "string");
    }
    assert_eq!(
        history["properties"]["limit"],
        json!({"type":"integer","minimum":1,"maximum":9007199254740991u64,"default":500})
    );
    assert_eq!(
        history["properties"]["format"]["enum"],
        json!(["markdown", "txt", "json", "yaml"])
    );
    assert_eq!(history["properties"]["format"]["default"], "markdown");
    assert!(schema["properties"]["options"]["properties"]
        .get("formats")
        .is_none());
    assert_eq!(schema["allOf"][0]["then"]["required"], json!(["options"]));
    assert_eq!(
        schema["allOf"][0]["then"]["properties"]["options"]["required"],
        json!(["history_export"])
    );
    assert_eq!(
        schema["allOf"][0]["else"]["properties"]["options"]["properties"]["history_export"],
        false
    );
    let call = parse("submit_task", &history_input(json!({"chat":"alice"}))).unwrap();
    assert!(args.authorize(&call));
    for denied in [
        Args::default(),
        Args {
            tasks: true,
            task_allow_artifact_read: true,
            task_allow_media_write: true,
            ..Default::default()
        },
        Args {
            task_kind: vec![Kind::ExportHistory],
            ..Default::default()
        },
    ] {
        assert!(!denied.authorize(&call));
    }
    let Call::Submit { task, .. } = call else {
        panic!()
    };
    let request = task.options.history_export.unwrap();
    assert_eq!(request.limit, 500);
    assert_eq!(
        request.format,
        crate::service::history_export::Format::Markdown
    );
    assert!(request.since.is_none() && request.until.is_none());
}

#[test]
fn history_parse_reuses_validation_and_rejects_unknown_or_cross_kind_fields() {
    for format in ["markdown", "txt", "json", "yaml"] {
        for limit in [1u64, 10001, 9007199254740991] {
            assert!(parse(
                "submit_task",
                &history_input(json!({"chat":"alice","format":format,"limit":limit,
                "since":"2026-09-18 23:59:59","until":"2026-09-18"}))
            )
            .is_ok());
        }
    }
    for request in [
        json!({}),
        json!(null),
        json!({"chat":""}),
        json!({"chat":"  "}),
        json!({"chat":"a\nb"}),
        json!({"chat":"中".repeat(86)}),
        json!({"chat":"a".repeat(257)}),
        json!({"chat":"alice","since":"123"}),
        json!({"chat":"alice","until":"2026-02-30"}),
        json!({"chat":"alice","since":"2026-09-19","until":"2026-09-18"}),
        json!({"chat":"alice","format":"html"}),
        json!({"chat":"alice","format":["json"]}),
        json!({"chat":"alice","formats":["json"]}),
        json!({"chat":"alice","offset":0}),
        json!({"chat":"alice","output":"C:/private"}),
        json!({"chat":"alice","path":"C:/private"}),
        json!({"chat":"alice","command":"cmd.exe"}),
        json!({"chat":"alice","debug_source":true}),
        json!({"chat":"alice","authorize_export":true}),
    ] {
        assert!(
            parse("submit_task", &history_input(request.clone())).is_err(),
            "{request}"
        );
    }
    for limit in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("500"),
        json!(true),
        json!(9007199254740992u64),
        json!(u64::MAX),
    ] {
        assert!(parse(
            "submit_task",
            &history_input(json!({"chat":"alice","limit":limit}))
        )
        .is_err());
    }
    for (key, value) in [
        ("users", json!(["alice"])),
        ("formats", json!(["json"])),
        ("include_images", json!(false)),
        ("include_sns", json!(true)),
        ("include_sns_media", json!(true)),
        ("allow_missing_media", json!(true)),
        ("authorize_memory_scan", json!(true)),
        ("dry_run", json!(true)),
        ("max_media_bytes", json!(1)),
        ("max_total_media_bytes", json!(67108864)),
    ] {
        let mut input = history_input(json!({"chat":"alice"}));
        input["options"][key] = value;
        assert!(parse("submit_task", &input).is_err(), "{key}");
    }
    for kind in [
        "export_all",
        "wechat_decrypt",
        "wechat_keys",
        "image_key",
        "decode_images",
        "sns_decrypt",
    ] {
        let mut input = history_input(json!({"chat":"alice"}));
        input["kind"] = json!(kind);
        assert!(parse("submit_task", &input).is_err());
    }
    assert!(parse(
        "submit_task",
        &json!({"idempotency_key":"a".repeat(64),"kind":"export_history"})
    )
    .is_err());
}

#[test]
fn history_invalid_input_and_host_denial_precede_account_access() {
    let args = Args {
        tasks: true,
        ..Default::default()
    };
    let account = Account::new(true);
    let invalidated = Cell::new(false);
    let io = tokio::runtime::Runtime::new().unwrap();
    let mut adapter = Adapter {
        query: |_request| Ok(Response::ok(json!({}))),
        account: &account,
        invalidated: &invalidated,
        io: &io,
        args: &args,
    };
    let reply = adapter
        .dispatch_task(
            "submit_task",
            &history_input(json!({"chat":"alice"})),
            &CallContext::default(),
        )
        .unwrap();
    assert_eq!(
        reply["structuredContent"]["error"]["code"],
        "host_forbidden"
    );
    assert!(matches!(
        adapter.dispatch_task(
            "submit_task",
            &history_input(json!({"chat":"alice","limit":0})),
            &CallContext::default()
        ),
        Err(DispatchError::InvalidArguments)
    ));
    assert!(account.runtime().is_none());
    assert!(!invalidated.get());
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
        json!(["wechat_decrypt", "export_history", "chat_plan"])
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
        "unsupported_task",
        "plan_ref_unavailable",
        "plan_ref_changed",
        "plan_selection_invalid",
        "invalid_page",
        "unauthorized",
        "invalid_artifact_request",
        "task_not_terminal",
        "result_unavailable",
        "artifact_unavailable",
        "artifact_changed",
        "artifact_unsafe",
        "artifact_busy",
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
    assert_eq!(tools.len(), crate::mcp::protocol::tools().len() + 8);
    for name in [
        "list_task_artifacts",
        "read_task_artifact",
        "read_chat_plan",
    ] {
        let annotations = &tools.iter().find(|tool| tool["name"] == name).unwrap()["annotations"];
        assert_eq!(annotations["readOnlyHint"], true);
        assert_eq!(annotations["destructiveHint"], false);
        assert_eq!(annotations["openWorldHint"], false);
    }
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

#[test]
fn export_budget_schema_and_parsing_preserve_types_and_host_consent() {
    let args = all_permissions();
    let tools = args.tools();
    let schema = &tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap()
        .input_schema;
    let options = &schema["properties"]["options"]["properties"];
    assert_eq!(options["dry_run"]["type"], "boolean");
    assert_eq!(options["dry_run"]["default"], false);
    assert_eq!(options["max_media_bytes"]["type"], "integer");
    assert_eq!(options["max_media_bytes"]["maximum"], 524288000u64);
    assert_eq!(options["max_total_media_bytes"]["type"], "integer");
    assert_eq!(options["max_total_media_bytes"]["maximum"], 17179869184u64);
    assert_eq!(
        schema["allOf"][0]["if"]["properties"]["kind"]["const"],
        "export_all"
    );
    let parse_options = |kind: &str, options: Value| {
        parse(
            "submit_task",
            &json!({
                "idempotency_key":"a".repeat(64),"kind":kind,"options":options
            }),
        )
    };
    for options in [
        json!({"dry_run":true}),
        json!({"dry_run":true,"include_images":false}),
        json!({"max_media_bytes":1,"max_total_media_bytes":1}),
        json!({"max_media_bytes":524288000u64,"max_total_media_bytes":17179869184u64}),
    ] {
        let call = parse_options("export_all", options).unwrap();
        assert!(args.authorize(&call));
        let mut denied = args.clone();
        denied.task_allow_media_write = false;
        assert!(!denied.authorize(&call));
    }
    for options in [
        json!({"dry_run":"true"}),
        json!({"dry_run":true,"include_sns":true}),
        json!({"include_images":false,"max_media_bytes":1}),
        json!({"include_images":false,"max_total_media_bytes":67108864}),
        json!({"max_media_bytes":0}),
        json!({"max_media_bytes":524288001u64}),
        json!({"max_media_bytes":-1}),
        json!({"max_media_bytes":1.5}),
        json!({"max_media_bytes":"1"}),
        json!({"max_media_bytes":true}),
        json!({"max_media_bytes":1e30}),
        json!({"max_total_media_bytes":17179869185u64}),
        json!({"max_total_media_bytes":67108863}),
        json!({"max_media_bytes":2,"max_total_media_bytes":1}),
    ] {
        assert!(
            parse_options("export_all", options.clone()).is_err(),
            "{options}"
        );
    }
    for kind in [
        "wechat_decrypt",
        "wechat_keys",
        "image_key",
        "decode_images",
        "sns_decrypt",
    ] {
        for options in [
            json!({"dry_run":true}),
            json!({"max_media_bytes":1}),
            json!({"max_total_media_bytes":67108864}),
        ] {
            assert!(parse_options(kind, options).is_err());
        }
    }
    let limited = Args {
        tasks: true,
        task_kind: vec![Kind::WechatDecrypt],
        ..Default::default()
    };
    let tools = limited.tools();
    let options = &tools
        .iter()
        .find(|tool| tool.name == "submit_task")
        .unwrap()
        .input_schema["properties"]["options"]["properties"];
    for key in ["dry_run", "max_media_bytes", "max_total_media_bytes"] {
        assert!(options.get(key).is_none());
    }
}

#[test]
fn artifact_arguments_are_bounded_opaque_and_read_only_authorized() {
    let id = "a".repeat(64);
    let artifact = "b".repeat(64);
    let args = Args {
        tasks: true,
        task_allow_artifact_read: true,
        ..Default::default()
    };
    let list = parse("list_task_artifacts", &json!({"id":id})).unwrap();
    assert!(
        matches!(&list, Call::TaskArtifacts { id:actual, offset:0, limit:50 } if actual == &id)
    );
    let read = parse(
        "read_task_artifact",
        &json!({"id":id,"artifact_id":artifact}),
    )
    .unwrap();
    assert!(
        matches!(&read, Call::ReadTaskArtifact { id:actual, artifact_id:a, offset:0, max_bytes:1048576 } if actual == &id && a == &artifact)
    );
    for call in [&list, &read] {
        assert!(args.authorize(call));
        assert!(!Args::default().authorize(call));
        assert!(!Args {
            tasks: true,
            ..Default::default()
        }
        .authorize(call));
        assert!(!Args {
            task_allow_artifact_read: true,
            ..Default::default()
        }
        .authorize(call));
    }
    assert!(matches!(
        parse(
            "read_task_artifact",
            &json!({"id":id,"artifact_id":artifact,"offset":7,"max_bytes":1})
        )
        .unwrap(),
        Call::ReadTaskArtifact {
            offset: 7,
            max_bytes: 1,
            ..
        }
    ));
    for (name, mut base) in [
        ("list_task_artifacts", json!({"id":id})),
        (
            "read_task_artifact",
            json!({"id":id,"artifact_id":artifact}),
        ),
    ] {
        for (key, value) in [
            ("path", json!("C:/private")),
            ("command", json!("cmd.exe")),
            ("runtime_id", json!("other")),
            ("authorized", json!(true)),
            ("offset", json!(-1)),
            ("offset", json!(1.5)),
            ("offset", json!("0")),
            ("offset", json!(1e30)),
            ("id", json!("A".repeat(64))),
        ] {
            let mut bad = base.clone();
            bad[key] = value;
            assert!(parse(name, &bad).is_err(), "{name}: {bad}");
        }
        let (key, too_large) = if name == "list_task_artifacts" {
            ("limit", 101)
        } else {
            ("max_bytes", 1048577)
        };
        for value in [
            json!(0),
            json!(-1),
            json!(too_large),
            json!("1"),
            json!(true),
            json!(1.5),
            json!(u64::MAX),
        ] {
            base[key] = value;
            assert!(parse(name, &base).is_err());
        }
    }
    for value in [
        json!("../a"),
        json!("B".repeat(64)),
        json!(true),
        json!(null),
    ] {
        assert!(parse("read_task_artifact", &json!({"id":id,"artifact_id":value})).is_err());
    }
    assert!(parse(
        "read_task_artifact",
        &json!({"id":id,"artifact_id":artifact,"offset":u64::MAX})
    )
    .is_err());
}

fn artifact_context(max_bytes: usize, response_id: Value) -> CallContext {
    let mut budget = CallContext::default().budget().unwrap();
    budget.max_response_bytes = max_bytes;
    budget.response_id = response_id;
    CallContext::from_budget(budget).unwrap()
}

fn artifact_block(offset: u64, data: &[u8], size: u64) -> Value {
    use base64::Engine;
    json!({
        "version":1,"task_id":"a".repeat(64),"artifact_id":"b".repeat(64),
        "offset":offset,"bytes_read":data.len(),"next_offset":offset + data.len() as u64,
        "size":size,"sha256":"c".repeat(64),"encoding":"base64",
        "data_base64":base64::engine::general_purpose::STANDARD.encode(data),
        "eof":offset + data.len() as u64 == size
    })
}

#[test]
fn artifact_chunks_fit_real_encoded_frames_and_round_trip_multiple_pages() {
    use base64::Engine;
    let data: Vec<u8> = (0..1048576 + 113)
        .map(|index| (index % 256) as u8)
        .collect();
    for (frame, response_id) in [
        (2048, json!(1)),
        (4096, json!("\\\"".repeat(64))),
        (1048576, json!(1)),
        (16 * 1048576, json!(null)),
    ] {
        let context = artifact_context(frame, response_id.clone());
        let mut offset = 0;
        let mut collected = Vec::new();
        while offset < data.len() {
            let mut call = parse(
                "read_task_artifact",
                &json!({
                    "id":"a".repeat(64),"artifact_id":"b".repeat(64),"offset":offset
                }),
            )
            .unwrap();
            fit_artifact_read(&mut call, &context).unwrap();
            let Call::ReadTaskArtifact { max_bytes, .. } = call else {
                panic!()
            };
            assert!((1..=1048576).contains(&max_bytes));
            if frame == 1048576 {
                assert!(max_bytes < 1048576);
            }
            let end = (offset + max_bytes as usize).min(data.len());
            let result = bounded_content(
                artifact_block(offset as u64, &data[offset..end], data.len() as u64),
                &context,
            )
            .unwrap();
            assert!(frame_size(&result, &response_id).unwrap() <= frame);
            let block = &result["structuredContent"];
            assert_eq!(block["next_offset"], end);
            assert_eq!(block["eof"], end == data.len());
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(block["data_base64"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                decoded.len(),
                block["bytes_read"].as_u64().unwrap() as usize
            );
            collected.extend(decoded);
            offset = block["next_offset"].as_u64().unwrap() as usize;
        }
        assert_eq!(collected, data);
    }
}

#[test]
fn artifact_frame_minimum_tiny_requests_eof_and_oversize_are_not_truncated() {
    use base64::Engine;
    let mut invalid_budget = CallContext::default().budget().unwrap();
    invalid_budget.max_response_bytes = 128;
    assert!(matches!(
        CallContext::from_budget(invalid_budget),
        Err(DispatchError::Unavailable)
    ));
    let context = artifact_context(1024, json!(1));
    let mut call = parse(
        "read_task_artifact",
        &json!({"id":"a".repeat(64),"artifact_id":"b".repeat(64)}),
    )
    .unwrap();
    fit_artifact_read(&mut call, &context).unwrap();
    let Call::ReadTaskArtifact { max_bytes, .. } = call else {
        unreachable!()
    };
    assert!((1..1024).contains(&max_bytes));
    let bytes = vec![0; max_bytes as usize];
    let result = bounded_content(artifact_block(0, &bytes, bytes.len() as u64), &context).unwrap();
    assert!(frame_size(&result, &json!(1)).unwrap() <= 1024);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result["structuredContent"]["data_base64"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    for (frame, id) in [
        (1024, json!("x".repeat(1024))),
        (2048, json!("x".repeat(2048))),
    ] {
        let context = artifact_context(frame, id);
        let mut call = parse(
            "read_task_artifact",
            &json!({"id":"a".repeat(64),"artifact_id":"b".repeat(64)}),
        )
        .unwrap();
        assert!(matches!(
            fit_artifact_read(&mut call, &context),
            Err(DispatchError::ResultLimit)
        ));
    }
    let context = artifact_context(4096, json!(1));
    for requested in [1, 2, 3, 4, 1048576] {
        let mut call = parse(
            "read_task_artifact",
            &json!({"id":"a".repeat(64),"artifact_id":"b".repeat(64),"max_bytes":requested}),
        )
        .unwrap();
        fit_artifact_read(&mut call, &context).unwrap();
        let Call::ReadTaskArtifact { max_bytes, .. } = call else {
            panic!()
        };
        assert!(max_bytes <= requested);
        let result = bounded_content(
            artifact_block(0, &vec![255; max_bytes as usize], max_bytes as u64),
            &context,
        )
        .unwrap();
        assert!(frame_size(&result, &json!(1)).unwrap() <= 4096);
    }
    for offset in [0, 1048576] {
        let result = bounded_content(artifact_block(offset, &[], offset), &context).unwrap();
        assert_eq!(result["structuredContent"]["data_base64"], "");
        assert_eq!(result["structuredContent"]["next_offset"], offset);
        assert_eq!(result["structuredContent"]["eof"], true);
    }
    assert!(matches!(
        bounded_content(artifact_block(0, &vec![0; 4096], 4096), &context),
        Err(DispatchError::ResultLimit)
    ));
}

#[test]
fn artifact_host_flag_controls_discovery_and_direct_dispatch_before_account_io() {
    use clap::Parser;
    #[derive(Parser)]
    struct Host {
        #[command(flatten)]
        args: Args,
    }
    assert!(Host::try_parse_from(["mcp", "--task-allow-artifact-read"]).is_err());
    let host = Host::try_parse_from(["mcp", "--tasks", "--task-allow-artifact-read"]).unwrap();
    assert_eq!(host.args.tools().len(), 7);
    assert!(host.args.task_kind.is_empty());
    assert!(!host.args.task_allow_media_write);
    assert!(host
        .args
        .tools()
        .iter()
        .all(|tool| tool.name != "submit_task"));
    let args = Args {
        tasks: true,
        ..Default::default()
    };
    assert!(args
        .tools()
        .iter()
        .all(|tool| !matches!(tool.name, "list_task_artifacts" | "read_task_artifact")));
    let account = Account::new(true);
    let invalidated = Cell::new(false);
    let io = tokio::runtime::Runtime::new().unwrap();
    let mut adapter = Adapter {
        query: |_request| Ok(Response::ok(json!({}))),
        account: &account,
        invalidated: &invalidated,
        io: &io,
        args: &args,
    };
    for (name, input) in [
        ("list_task_artifacts", json!({"id":"a".repeat(64)})),
        (
            "read_task_artifact",
            json!({"id":"a".repeat(64),"artifact_id":"b".repeat(64)}),
        ),
    ] {
        let reply = adapter
            .dispatch_task(name, &input, &CallContext::default())
            .unwrap();
        assert_eq!(
            reply["structuredContent"]["error"]["code"],
            "host_forbidden"
        );
        assert_eq!(reply["isError"], true);
        let mut model_grant = input;
        model_grant["task_allow_artifact_read"] = json!(true);
        assert!(matches!(
            adapter.dispatch_task(name, &model_grant, &CallContext::default()),
            Err(DispatchError::InvalidArguments)
        ));
    }
    assert!(account.runtime().is_none());
}
