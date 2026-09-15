use super::*;
use std::io::{BufReader, Cursor};

#[test]
fn contacts_legacy_view_is_internal_and_cli_wire_stays_unchanged() {
    let tool = tools()
        .into_iter()
        .find(|t| t.name == "get_contacts")
        .unwrap();
    assert!(tool.input_schema["properties"].get("legacy_view").is_none());
    for value in [json!(true), json!(false), Value::Null] {
        assert!(route("get_contacts", &json!({"legacy_view": value})).is_err());
    }
    let request = route("get_contacts", &json!({})).unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({"cmd":"contacts","limit":50,"legacy_view":true})
    );
    for wire in [
        json!({"cmd":"contacts"}),
        json!({"cmd":"contacts","legacy_view":false}),
    ] {
        let request: Request = serde_json::from_value(wire).unwrap();
        assert!(matches!(
            &request,
            Request::Contacts {
                legacy_view: false,
                limit: 50,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"cmd":"contacts","limit":50})
        );
    }
}

fn synthetic(_: Request) -> Result<Response, DispatchError> {
    Ok(Response::ok(
        json!({"synthetic":true,"text":"line1\nline2"}),
    ))
}
fn send<D: Dispatcher>(p: &mut Protocol<D>, v: Value) -> Option<Value> {
    p.handle(&serde_json::to_vec(&v).unwrap())
}
fn ready<D: Dispatcher>(p: &mut Protocol<D>) {
    let response = send(p, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}})).unwrap();
    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(p.phase(), Phase::AwaitingInitialized);
    assert!(send(
        p,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .is_none());
    assert_eq!(p.phase(), Phase::Ready);
}
fn call(name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":name,"arguments":args}})
}

#[test]
fn lifecycle_negotiates_and_gates_queries() {
    let mut p = Protocol::new(synthetic);
    assert!(send(
        &mut p,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .is_none());
    assert_eq!(p.phase(), Phase::New);
    assert_eq!(
        send(&mut p, call("get_contacts", json!({}))).unwrap()["error"]["code"],
        -32002
    );
    let init = json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":"unsupported","capabilities":{},"clientInfo":{"name":"x","version":"1"}}});
    assert_eq!(
        send(&mut p, init.clone()).unwrap()["result"]["protocolVersion"],
        PROTOCOL_VERSION
    );
    assert_eq!(
        send(&mut p, call("get_contacts", json!({}))).unwrap()["error"]["code"],
        -32002
    );
    assert_eq!(send(&mut p, init).unwrap()["error"]["code"], -32600);
}

#[test]
fn invalid_initialize_does_not_change_phase() {
    let mut p = Protocol::new(synthetic);
    assert_eq!(
        send(
            &mut p,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
        )
        .unwrap()["error"]["code"],
        -32602
    );
    assert_eq!(p.phase(), Phase::New);
    ready(&mut p);
}

#[test]
fn request_ids_are_preserved_and_invalid_envelopes_rejected() {
    let mut p = Protocol::new(synthetic);
    for id in [
        json!(0),
        json!(-9),
        json!(u64::MAX),
        json!(""),
        json!("中文-id"),
    ] {
        let r = send(&mut p, json!({"jsonrpc":"2.0","id":id,"method":"ping"})).unwrap();
        assert_eq!(r["id"], id);
        assert_eq!(r["result"], json!({}));
    }
    for id in [Value::Null, json!(true), json!([]), json!({}), json!(1.5)] {
        let r = send(&mut p, json!({"jsonrpc":"2.0","id":id,"method":"ping"})).unwrap();
        assert_eq!(r["error"]["code"], -32600);
        assert!(r["id"].is_null());
    }
    for v in [
        json!([]),
        json!([{"jsonrpc":"2.0","method":"ping","id":1}]),
        json!({"jsonrpc":"1.0","method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"result":{}}),
    ] {
        assert_eq!(send(&mut p, v).unwrap()["error"]["code"], -32600);
    }
}

#[test]
fn notifications_never_reply_or_dispatch() {
    let mut p =
        Protocol::new(|_| -> Result<Response, DispatchError> { panic!("notification dispatched") });
    ready(&mut p);
    for (method, params) in [
        ("tools/call", json!({"name":"get_contacts"})),
        ("unknown", json!({})),
        ("ping", json!([])),
        ("initialize", json!({})),
    ] {
        assert!(send(
            &mut p,
            json!({"jsonrpc":"2.0","method":method,"params":params})
        )
        .is_none());
    }
}

#[test]
fn schemas_match_real_ipc_and_reject_unimplemented_arguments() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/mcp-protocol/routes.json"
    ))
    .unwrap();
    assert_eq!(fixtures.as_array().unwrap().len(), tools().len());
    for case in fixtures.as_array().unwrap() {
        let request = route(case["name"].as_str().unwrap(), &case["arguments"]).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), case["ipc"]);
    }
    for args in [
        json!({"chat_name":"x","start_time":"not-a-date"}),
        json!({"chat_name":"x","limit":0}),
        json!({"chat_name":"x","limit":501}),
        json!({"chat_name":"x","offset":-1}),
        json!({"chat_name":"x","since":5,"until":4}),
        json!({"chat_name":"x","msg_type":null}),
        json!({"chat_name":" "}),
        json!({"chat_name":true}),
        json!({"chat_name":"x","cmd":"extract"}),
        json!({}),
    ] {
        assert!(route("get_chat_history", &args).is_err(), "{args}");
    }
    assert!(route("search_messages", &json!({"keyword":"x","chats":[1]})).is_err());
    assert!(route("get_contacts", &json!({"query":"a".repeat(4097)})).is_err());
    assert!(route("transcribe_voice", &json!({})).is_err());
}

#[test]
fn lists_only_registered_tools_and_returns_text_content() {
    let mut p = Protocol::new(synthetic);
    ready(&mut p);
    let list = send(
        &mut p,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 17);
    assert!(list["result"].get("nextCursor").is_none());
    let r = send(&mut p, call("get_contacts", json!({}))).unwrap();
    assert_eq!(r["result"]["isError"], false);
    let data: Value =
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["synthetic"], true);
}

#[test]
fn voice_routes_preserve_media_ids_and_reject_host_configuration() {
    for name in ["decode_voice", "transcribe_voice"] {
        let args = json!({"chat_name":"peer","local_id":700});
        assert_eq!(
            serde_json::to_value(route(name, &args).unwrap()).unwrap(),
            json!({"cmd":name,"chat":"peer","local_id":700})
        );
        for field in [
            "output_root",
            "backend",
            "allow_upload",
            "api_key_file",
            "voice_cache_file",
            "source",
            "create_time",
        ] {
            let mut injected = args.clone();
            injected[field] = json!("not accepted");
            assert!(route(name, &injected).is_err(), "{name}: {field}");
        }
        for id in [0, -1] {
            assert!(route(name, &json!({"chat_name":"peer","local_id":id})).is_err());
        }
        let tool = tools().into_iter().find(|tool| tool.name == name).unwrap();
        assert!(!tool.read_only());
        assert_eq!(tool.open_world(), name == "transcribe_voice");
    }
}

#[test]
fn text_result_budget_counts_exact_id_envelope_and_json_escaping() {
    for id in [
        json!(700),
        json!("request\"\n中文"),
        json!("long".repeat(200)),
    ] {
        let text = "结果\n\"\\\u{0000}";
        let bytes =
            serde_json::to_vec(&result(id.clone(), text_result(text.into(), false))).unwrap();
        let mut context = CallContext {
            response_id: id,
            max_response_bytes: bytes.len(),
            ..CallContext::default()
        };
        assert_eq!(context.check_text_result(text), Ok(()));
        context.max_response_bytes -= 1;
        assert_eq!(
            context.check_text_result(text),
            Err(DispatchError::ResultLimit)
        );
    }
}

#[test]
fn stdio_passes_actual_response_budget_before_voice_publication() {
    let publications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = publications.clone();
    let mut protocol = Protocol::new(Controlled(move |_: Request, context: &CallContext| {
        let text = "x".repeat(400);
        context.check_text_result(&text)?;
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Response::ok(json!({"mcp_text":text})))
    }));
    ready(&mut protocol);
    let mut request = call("decode_voice", json!({"chat_name":"peer","local_id":700}));
    request["id"] = json!("i".repeat(600));
    let mut input = serde_json::to_vec(&request).unwrap();
    input.push(b'\n');
    assert!(input.len() < 1024);
    let mut output = Vec::new();
    protocol
        .serve(std::io::Cursor::new(input), &mut output, 1024)
        .unwrap();
    let reply: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(reply["id"], request["id"]);
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(
        reply["result"]["content"][0]["text"],
        "Query result exceeds safe limit"
    );
    assert_eq!(publications.load(Ordering::SeqCst), 0);
}

#[test]
fn voice_results_require_completed_host_text_not_prepared_audio() {
    for name in ["decode_voice", "transcribe_voice"] {
        for data in [
            json!({"prepared_audio":{"silk_base64":"must-not-leak"}}),
            json!({"mcp_text":null}),
            json!({"mcp_text":42}),
        ] {
            let mut protocol = Protocol::new(move |_| Ok(Response::ok(data.clone())));
            ready(&mut protocol);
            let reply = send(
                &mut protocol,
                call(name, json!({"chat_name":"peer","local_id":700})),
            )
            .unwrap();
            assert_eq!(
                reply["result"],
                json!({"isError":true,"content":[{"type":"text","text":"Invalid query response"}]})
            );
        }
        let mut protocol =
            Protocol::new(|_| Ok(Response::ok(json!({"mcp_text":"完成\n原生结果"}))));
        ready(&mut protocol);
        let reply = send(
            &mut protocol,
            call(name, json!({"chat_name":"peer","local_id":700})),
        )
        .unwrap();
        assert_eq!(
            reply["result"],
            json!({"isError":false,"content":[{"type":"text","text":"完成\n原生结果"}]})
        );
    }
}

#[test]
fn attachment_tools_validate_identity_and_reject_paths_before_dispatch() {
    let mut protocol = Protocol::new(|_| -> Result<Response, DispatchError> {
        panic!("invalid attachment arguments must not dispatch")
    });
    ready(&mut protocol);
    for name in ["decode_file_message", "decode_record_item"] {
        let mut valid = json!({"chat_name":"synthetic","local_id":1});
        if name == "decode_record_item" {
            valid["item_index"] = json!(0);
        }
        let tool = tools().into_iter().find(|t| t.name == name).unwrap();
        assert_eq!(tool.input_schema["properties"]["create_time"]["default"], 0);
        let request = serde_json::to_value(route(name, &valid).unwrap()).unwrap();
        assert_eq!(request["create_time"], 0);
        for timestamp in [i64::MIN, 42, i64::MAX] {
            let mut explicit = valid.clone();
            explicit["create_time"] = json!(timestamp);
            explicit["local_id"] = json!(i64::MAX);
            if name == "decode_record_item" {
                explicit["item_index"] = json!(i64::MAX);
            }
            let expected = serde_json::to_value(route(name, &explicit).unwrap()).unwrap();
            let mut success = Protocol::new(|request| Ok(Response::ok(json!({"request":request}))));
            ready(&mut success);
            let reply = send(&mut success, call(name, explicit)).unwrap();
            assert_eq!(reply["result"]["isError"], false);
            let body: Value = serde_json::from_str(reply_text(&reply)).unwrap();
            assert_eq!(body["request"], expected);
            assert_eq!(expected["create_time"], timestamp);
        }
        let mut invalid = vec![json!({}), json!(null), json!([])];
        for required in tool.input_schema["required"].as_array().unwrap() {
            let mut missing = valid.clone();
            missing
                .as_object_mut()
                .unwrap()
                .remove(required.as_str().unwrap());
            invalid.push(missing);
        }
        for (field, values) in [
            (
                "local_id",
                vec![
                    json!(0),
                    json!(-1),
                    json!(1.5),
                    json!(u64::MAX),
                    json!("1"),
                    json!(null),
                ],
            ),
            (
                "create_time",
                vec![json!(1.5), json!(u64::MAX), json!(null), json!("0")],
            ),
            (
                "chat_name",
                vec![json!(" "), json!(true), json!("x".repeat(4097))],
            ),
            ("path", vec![json!("C:\\PRIVATE\\file")]),
            ("output", vec![json!("../PRIVATE")]),
            ("url", vec![json!("https://PRIVATE/upload")]),
            ("allow_upload", vec![json!(true)]),
            ("cmd", vec![json!("write")]),
        ] {
            for value in values {
                let mut args = valid.clone();
                args[field] = value;
                invalid.push(args);
            }
        }
        for index in [
            json!(-1),
            json!(0.5),
            json!(u64::MAX),
            json!("0"),
            json!(null),
        ] {
            let mut args = valid.clone();
            args["item_index"] = index;
            invalid.push(args);
        }
        for args in invalid {
            let reply = send(&mut protocol, call(name, args)).unwrap();
            assert_eq!(reply["error"]["code"], -32602);
            assert!(!reply.to_string().contains("PRIVATE"));
        }
        for failure in [
            Response::err("PRIVATE path/key"),
            Response::ok(json!({"exit_code":2,"text":"PRIVATE path/key"})),
            Response::ok(json!({"error":"PRIVATE path/key"})),
        ] {
            let mut failed = Protocol::new(move |_| Ok(failure.clone()));
            ready(&mut failed);
            let reply = send(&mut failed, call(name, valid.clone())).unwrap();
            assert_eq!(reply_text(&reply), "Query failed");
            assert_eq!(reply["result"]["isError"], true);
        }
    }
    for name in ["decode_voice", "transcribe_voice"] {
        assert!(route(name, &json!({})).is_err());
    }
}

#[test]
fn readonly_extensions_validate_arguments_and_keep_safe_errors() {
    for (name, args) in [
        ("get_contact_tags", json!({"output":"forbidden"})),
        ("get_tag_members", json!({})),
        ("get_tag_members", json!({"tag_name":"x".repeat(4097)})),
        ("decode_refer", json!({"chat_name":" ","local_id":1})),
        ("decode_refer", json!({"chat_name":"x","local_id":1.5})),
        ("get_voice_messages", json!({"chat_name":"x","limit":501})),
        ("get_voice_messages", json!({"chat_name":"x","offset":-1})),
        (
            "get_voice_messages",
            json!({"chat_name":"x","since":2,"until":1}),
        ),
        (
            "get_voice_messages",
            json!({"chat_name":"x","start_time":"invalid"}),
        ),
    ] {
        assert!(route(name, &args).is_err(), "{name}: {args}");
    }
    let request = route(
        "get_voice_messages",
        &json!({
            "chat_name":"x", "start_time":"", "end_time":"", "offset":2
        }),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({"cmd":"voice_messages","chat":"x","limit":20,"offset":2})
    );
    for (name, args) in [
        ("get_contact_tags", json!({})),
        ("get_tag_members", json!({"tag_name":"x"})),
        ("decode_refer", json!({"chat_name":"x","local_id":1})),
        ("get_voice_messages", json!({"chat_name":"x"})),
    ] {
        let mut protocol = Protocol::new(|_| {
            Ok(Response::ok(json!({
                "exit_code":2,"text":"synthetic-private-error"
            })))
        });
        ready(&mut protocol);
        let response = send(&mut protocol, call(name, args)).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["content"][0]["text"], "Query failed");
        assert!(!response.to_string().contains("synthetic-private-error"));
    }
}

#[test]
fn dispatch_errors_are_mapped_without_transport_error_details() {
    for (response, code, is_error) in [
        (Ok(Response::err("synthetic failure")), None, true),
        (Err(DispatchError::Unavailable), None, true),
        (Err(DispatchError::Internal), Some(-32603), false),
    ] {
        let mut p = Protocol::new(move |_| response.clone());
        ready(&mut p);
        let r = send(&mut p, call("get_contacts", json!({}))).unwrap();
        if let Some(code) = code {
            assert_eq!(r["error"]["code"], code);
        } else {
            assert_eq!(r["result"]["isError"], is_error);
        }
    }
}

#[test]
fn method_and_argument_errors() {
    let mut p = Protocol::new(synthetic);
    ready(&mut p);
    for (request, code) in [
        (call("missing", json!({})), -32602),
        (call("get_contacts", json!([])), -32602),
        (json!({"jsonrpc":"2.0","id":2,"method":"unknown"}), -32601),
        (
            json!({"jsonrpc":"2.0","id":2,"method":"ping","params":null}),
            -32602,
        ),
        (
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"cursor":"x"}}),
            -32602,
        ),
    ] {
        assert_eq!(send(&mut p, request).unwrap()["error"]["code"], code);
    }
}

#[test]
fn framed_output_is_only_json_rpc_with_no_notification_lines() {
    let input = b"{bad}\n\xff\n\n{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\r\n";
    let mut p = Protocol::new(synthetic);
    let mut output = Vec::new();
    p.serve(
        BufReader::with_capacity(2, Cursor::new(input)),
        &mut output,
        DEFAULT_MAX_FRAME_BYTES,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    let replies: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 4);
    assert!(replies.iter().all(|r| r["jsonrpc"] == "2.0"));
    for r in &replies[..3] {
        assert_eq!(r["error"]["code"], -32700);
    }
    assert_eq!(replies[3]["id"], 7);
}

#[test]
fn frame_limits_and_truncation_are_bounded() {
    assert_eq!(
        read_frame(&mut Cursor::new(b"1234\n"), 4).unwrap().unwrap(),
        b"1234"
    );
    assert_eq!(
        read_frame(&mut Cursor::new(b"12345\n"), 4)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        read_frame(&mut Cursor::new(b"1234"), 4).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert!(read_frame(&mut Cursor::new(b""), 4).unwrap().is_none());
    let mut p = Protocol::new(synthetic);
    assert!(p.serve(Cursor::new(b""), Vec::new(), 0).is_err());
    let mut output = Vec::new();
    let ping = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
    // 输入恰好可容纳，但初始化后查询的大结果不得输出半帧。
    ready(&mut p);
    let request = serde_json::to_string(&call("get_contacts", json!({}))).unwrap() + "\n";
    let mut large = Protocol::new(|_| Ok(Response::ok(json!({"large":"x".repeat(10000)}))));
    ready(&mut large);
    assert!(large
        .serve(Cursor::new(request.as_bytes()), &mut output, 200)
        .is_err());
    assert!(output.is_empty());
    p.serve(Cursor::new(ping), &mut output, 200).unwrap();
    assert!(output.ends_with(b"\n"));
}

#[test]
fn writer_failure_is_propagated() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "synthetic"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut p = Protocol::new(synthetic);
    assert_eq!(
        p.serve(Cursor::new(b"{}\n"), Broken, 1000)
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn legacy_time_and_type_arguments_map_without_ambiguity() {
    let r = route("get_chat_history", &json!({"chat_name":"synthetic","start_time":"2024-02-29","end_time":"2024-02-29","oldest_first":false,"msg_types":["FILE","app"]})).unwrap();
    let r = serde_json::to_value(r).unwrap();
    let start = Local
        .with_ymd_and_hms(2024, 2, 29, 0, 0, 0)
        .single()
        .unwrap()
        .timestamp();
    let end = Local
        .with_ymd_and_hms(2024, 2, 29, 23, 59, 59)
        .single()
        .unwrap()
        .timestamp();
    assert_eq!(r["since"], start);
    assert_eq!(r["until"], end);
    assert_eq!(r["msg_type"], 49);
    for value in [json!(null), json!([])] {
        assert!(route(
            "get_chat_history",
            &json!({"chat_name":"s","msg_types":value})
        )
        .is_ok());
    }
    for args in [
        json!({"chat_name":"s","oldest_first":1}),
        json!({"chat_name":"s","msg_types":["image","text"],"msg_type":3}),
        json!({"chat_name":"s","msg_types":["secret_bad_type"]}),
        json!({"chat_name":"s","start_time":"2023-02-29"}),
        json!({"chat_name":"s","start_time":"2024-02-29","since":1}),
        json!({"chat_name":"s","start_time":"2024-03-01","end_time":"2024-02-29"}),
        json!({"chat_name":"s","msg_types":["text"],"msg_type":1}),
    ] {
        assert!(route("get_chat_history", &args).is_err());
    }
    assert_eq!(
        parse_legacy_time("2024-02-29 01:02", false).unwrap(),
        parse_legacy_time("2024-02-29 01:02:00", false).unwrap()
    );
}

#[test]
fn shared_type_projection_preserves_mcp_only_aliases_and_cli_only_rejections() {
    for (label, expected) in [
        (" TEXT ", 1),
        ("IMAGE", 3),
        ("VOICE", 34),
        ("NAMECARD", 42),
        ("VIDEO", 43),
        ("EMOJI", 47),
        ("LOCATION", 48),
        ("APP", 49),
        ("FILE", 49),
        ("VOIP", 50),
        ("SYSTEM", 10000),
    ] {
        let request = route(
            "get_chat_history",
            &json!({"chat_name":"synthetic", "msg_types":[label]}),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(request).unwrap()["msg_type"], expected);
    }
    for label in ["sticker", "call", "link", "49"] {
        assert!(matches!(
            route(
                "get_chat_history",
                &json!({"chat_name":"synthetic", "msg_types":[label]})
            ),
            Err("Unknown message type")
        ));
    }
    let request = route(
        "get_chat_history",
        &json!({"chat_name":"synthetic", "msg_type":(6_i64 << 32) | 49}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap()["msg_type"],
        (6_i64 << 32) | 49
    );
}

#[test]
fn image_route_never_accepts_host_paths_or_keys() {
    let input = json!({"chat_name":"peer","local_id":7});
    assert_eq!(
        serde_json::to_value(route("decode_image", &input).unwrap()).unwrap(),
        json!({"cmd":"decode_image","chat":"peer","local_id":7,"create_time":0})
    );
    for field in ["output_root", "image_key_file", "aes_key", "base_dir"] {
        let mut invalid = input.clone();
        invalid[field] = json!("untrusted-private-value");
        assert!(route("decode_image", &invalid).is_err());
    }
    for invalid in [
        json!({"chat_name":"peer","local_id":0}),
        json!({"chat_name":"peer","local_id":1,"create_time":-1}),
    ] {
        assert!(route("decode_image", &invalid).is_err());
    }
}

#[test]
fn history_multiple_types_and_oldest_page_reach_ipc() {
    let multi = serde_json::to_value(
        route(
            "get_chat_history",
            &json!({
                "chat_name":"s", "msg_types":["image","text","IMAGE","file","app"],
                "oldest_first":true,"offset":3,"limit":2,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(multi["msg_types"], json!([3, 1, 49]));
    assert_eq!(multi["oldest_first"], true);
    assert_eq!(multi["offset"], 3);
    assert_eq!(multi["limit"], 2);
    assert!(multi.get("msg_type").is_none());
    let oldest = serde_json::to_value(
        route(
            "get_chat_history",
            &json!({
                "chat_name":"s", "oldest_first":true,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(oldest["oldest_first"], true);
    assert!(oldest.get("msg_types").is_none());
}

#[test]
fn legacy_search_names_and_offset_are_applied_globally() {
    let mut p = Protocol::new(|request| {
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"cmd":"search","keyword":"synthetic","chats":["a","b"],"limit":4})
        );
        Ok(Response::ok(
            json!({"results":[{"timestamp":4},{"timestamp":3},{"timestamp":2},{"timestamp":1}],"count":4}),
        ))
    });
    ready(&mut p);
    let r = send(
        &mut p,
        call(
            "search_messages",
            json!({"keyword":"synthetic","chat_name":[" a ","b","a",""],"offset":2,"limit":2}),
        ),
    )
    .unwrap();
    let data: Value =
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["results"], json!([{"timestamp":2},{"timestamp":1}]));
    assert_eq!(data["count"], 2);
    for name in [json!(null), json!(" "), json!([])] {
        let request = route("search_messages", &json!({"keyword":"s","chat_name":name})).unwrap();
        assert!(serde_json::to_value(request)
            .unwrap()
            .get("chats")
            .is_none());
    }
    assert!(route(
        "search_messages",
        &json!({"keyword":"s","offset":9999,"limit":2})
    )
    .is_err());
    assert!(route(
        "search_messages",
        &json!({"keyword":"s","chat_name":"a","chats":["b"]})
    )
    .is_err());
}

fn session(username: &str, timestamp: i64, unread: i64) -> Value {
    json!({"username":username,"timestamp":timestamp,"unread":unread,"chat":username,"summary":"synthetic summary","is_group":false,"last_msg_type":"文本","last_sender":""})
}
fn reply_text(r: &Value) -> &str {
    r["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn polling_preserves_legacy_first_unread_and_changed_summary_semantics() {
    let mut responses = [
        json!({"sessions":[session("a",100,1),session("b",90,0)]}),
        json!({"sessions":[session("a",100,1),session("b",90,0)]}),
        json!({"sessions":[session("a",103,0),session("b",101,0)]}),
    ]
    .into_iter();
    let mut p = Protocol::new(move |request| {
        assert!(matches!(request, Request::Sessions { limit: 10001, .. }));
        Ok(Response::ok(responses.next().unwrap()))
    });
    ready(&mut p);
    let first = send(&mut p, call("get_new_messages", json!({}))).unwrap();
    assert!(reply_text(&first).starts_with("当前 1 个未读会话"));
    assert!(!reply_text(&first).contains("] b"));
    let same = send(&mut p, call("get_new_messages", json!({}))).unwrap();
    assert_eq!(reply_text(&same), "无新消息");
    let changed = send(&mut p, call("get_new_messages", json!({}))).unwrap();
    assert!(reply_text(&changed).starts_with("2 条新消息"));
    assert!(reply_text(&changed).find("] b").unwrap() < reply_text(&changed).find("] a").unwrap());
}

#[test]
fn snapshots_are_isolated_and_invalid_or_failed_results_do_not_advance() {
    let mut step = 0;
    let mut p = Protocol::new(move |_| {
        step += 1;
        Ok(match step {
            1 => Response::ok(json!({"sessions":[session("a",1,1)]})),
            2 => Response::err("private message keys=synthetic-secret"),
            3 => Response::ok(json!({"sessions":[session("a",2,1),{}]})),
            _ => Response::ok(json!({"sessions":[session("a",2,1)]})),
        })
    });
    ready(&mut p);
    send(&mut p, call("get_new_messages", json!({})));
    for _ in 0..2 {
        let r = send(&mut p, call("get_new_messages", json!({}))).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(p.session_state.as_ref().unwrap()["a"], 1);
    }
    assert!(
        reply_text(&send(&mut p, call("get_new_messages", json!({}))).unwrap())
            .starts_with("1 条新消息")
    );
    let mut other = Protocol::new(|_| Ok(Response::ok(json!({"sessions":[session("a",2,1)]}))));
    ready(&mut other);
    assert!(
        reply_text(&send(&mut other, call("get_new_messages", json!({}))).unwrap())
            .starts_with("当前 1 个未读会话")
    );
}

#[test]
fn polling_rejects_truncated_snapshot_instead_of_losing_cursor_entries() {
    let mut p = Protocol::new(|_| {
        Ok(Response::ok(
            json!({"sessions":vec![session("a",1,1);MAX_CANDIDATES+1]}),
        ))
    });
    ready(&mut p);
    let r = send(&mut p, call("get_new_messages", json!({}))).unwrap();
    assert_eq!(reply_text(&r), "Query result exceeds safe limit");
    assert!(p.session_state.is_none());
}

#[test]
fn image_listing_admits_missing_metadata_and_never_decodes() {
    let mut p = Protocol::new(|request| {
        assert!(matches!(
            request,
            Request::Attachments {
                image_metadata: true,
                ..
            }
        ));
        Ok(Response::ok(
            json!({"attachments":[{"local_id":1,"timestamp":2,"attachment_id":"internal-handle","path":"private-path","md5":null,"size":null,"resource_status":"missing","size_status":"not_requested","size_kind":"encrypted_dat_metadata","binding":"exact_resource_standard_filename_metadata"}]}),
        ))
    });
    ready(&mut p);
    let r = send(&mut p, call("get_chat_images", json!({"chat_name":"s"}))).unwrap();
    let data: Value = serde_json::from_str(reply_text(&r)).unwrap();
    assert_eq!(
        data["images"],
        json!([{"local_id":1,"create_time":2,"md5":null,"size":null,"resource_status":"missing","size_status":"not_requested","size_kind":"encrypted_dat_metadata","binding":"exact_resource_standard_filename_metadata"}])
    );
    assert_eq!(data["partial_legacy_compatibility"], true);
    assert!(!reply_text(&r).contains("internal-handle"));
    assert!(!reply_text(&r).contains("private-path"));
}

#[test]
fn image_metadata_preserves_zero_and_ambiguity_and_rejects_invalid_values() {
    let row = json!({"md5":"0123456789abcdef0123456789abcdef","size":0,
        "resource_status":"found","size_status":"available",
        "size_kind":"encrypted_dat_metadata","binding":"exact_resource_standard_filename_metadata",
        "packed_info":"secret","path":"private"});
    let value = image_metadata(&row).unwrap();
    assert_eq!(value["size"], 0);
    assert!(value.get("packed_info").is_none());
    assert!(value.get("path").is_none());
    let mut ambiguous = row.clone();
    ambiguous["size"] = Value::Null;
    ambiguous["size_status"] = json!("ambiguous");
    assert_eq!(image_metadata(&ambiguous).unwrap()["md5"], row["md5"]);
    for (field, bad) in [
        ("md5", json!("guessed")),
        ("md5", Value::Null),
        ("size", json!(-1)),
        ("size", json!("0")),
        ("size", Value::Null),
        ("resource_status", json!("missing")),
        ("size_status", json!("missing")),
        ("size_kind", json!("decoded_image")),
        ("binding", json!("guessed")),
    ] {
        let mut bad_row = row.clone();
        bad_row[field] = bad;
        assert_eq!(
            image_metadata(&bad_row).unwrap_err(),
            DispatchError::InvalidResponse
        );
    }
    assert!(route(
        "get_chat_images",
        &json!({"chat_name":"peer","image_metadata":false})
    )
    .is_err());
    let plain: Request =
        serde_json::from_value(json!({"cmd":"attachments","chat":"peer"})).unwrap();
    assert!(matches!(
        plain,
        Request::Attachments {
            image_metadata: false,
            ..
        }
    ));
}

#[test]
fn errors_never_echo_backend_message_keys_or_decode_failure_text() {
    for response in [
        Response::err("message=PRIVATE keys=SECRET"),
        Response::ok(json!({"exit_code":2,"text":"message=PRIVATE keys=SECRET"})),
        Response::ok(json!({"exit_code":3,"status":"error","message":"PRIVATE keys=SECRET"})),
        Response::ok(json!({"error":"SECRET","message":"PRIVATE"})),
    ] {
        let mut p = Protocol::new(move |_| Ok(response.clone()));
        ready(&mut p);
        let r = send(
            &mut p,
            call("decode_transfer", json!({"chat_name":"s","local_id":1})),
        )
        .unwrap();
        assert_eq!(reply_text(&r), "Query failed");
        assert!(!r.to_string().contains("PRIVATE"));
        assert!(!r.to_string().contains("SECRET"));
        assert_eq!(r["result"]["isError"], true);
    }
}

#[test]
fn business_partial_and_refusal_are_not_transport_unavailability_or_success() {
    for (data, expected) in [
        (
            json!({"status":"partial","success":false,"message":"PRIVATE SECRET"}),
            "Operation partially completed; successful artifacts were preserved",
        ),
        (
            json!({"status":"refused","message":"PRIVATE SECRET"}),
            "Business request refused",
        ),
        (
            json!({"success":false,"message":"PRIVATE SECRET"}),
            "Query failed",
        ),
    ] {
        let mut protocol = Protocol::new(move |_| Ok(Response::ok(data.clone())));
        ready(&mut protocol);
        let response = send(
            &mut protocol,
            call("decode_transfer", json!({"chat_name":"s","local_id":1})),
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(reply_text(&response), expected);
        assert!(!response.to_string().contains("PRIVATE"));
        assert!(!response.to_string().contains("SECRET"));
    }
}

#[test]
fn key_store_diagnosis_is_static_not_backend_message_or_unavailability() {
    use crate::ipc::outcome::KeyStoreDiagnostic;
    for diagnostic in [
        KeyStoreDiagnostic::Missing,
        KeyStoreDiagnostic::Invalid,
        KeyStoreDiagnostic::WrongAccount,
        KeyStoreDiagnostic::LegacyMigrationRequired,
    ] {
        let mut response = Response::err("PRIVATE SECRET");
        response.data = json!({"error_code":diagnostic.code()});
        let mut protocol = Protocol::new(move |_| Ok(response.clone()));
        ready(&mut protocol);
        let result = send(
            &mut protocol,
            call("decode_transfer", json!({"chat_name":"s","local_id":1})),
        )
        .unwrap();
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(reply_text(&result), diagnostic.message());
        assert!(!result.to_string().contains("PRIVATE"));
        assert!(!result.to_string().contains("SECRET"));
    }
}

#[test]
fn cancelled_or_expired_context_never_starts_callback() {
    let mut p =
        Protocol::new(|_| -> Result<Response, DispatchError> { panic!("must not dispatch") });
    ready(&mut p);
    let token = CancellationToken::default();
    token.cancel();
    for (context, expected) in [
        (
            CallContext::new(token, Duration::from_secs(1)),
            "Query cancelled",
        ),
        (
            CallContext::new(CancellationToken::default(), Duration::ZERO),
            "Query timed out",
        ),
    ] {
        let r = p
            .handle_with_context(
                &serde_json::to_vec(&call("get_contacts", json!({}))).unwrap(),
                &context,
            )
            .unwrap();
        assert_eq!(reply_text(&r), expected);
    }
}

#[test]
fn cooperative_cancellation_is_injectable_and_late_success_is_discarded() {
    let token = CancellationToken::default();
    let context = CallContext::new(token.clone(), Duration::from_secs(2));
    let mut p = Protocol::new(Controlled(|_: Request, ctx: &CallContext| {
        assert!(ctx.remaining() <= Duration::from_secs(2));
        ctx.cancellation().cancel();
        Ok(Response::ok(json!({"sessions":[session("a",1,1)]})))
    }));
    ready(&mut p);
    let r = p
        .handle_with_context(
            &serde_json::to_vec(&call("get_new_messages", json!({}))).unwrap(),
            &context,
        )
        .unwrap();
    assert_eq!(reply_text(&r), "Query cancelled");
    assert!(token.is_cancelled());
    assert!(p.session_state.is_none());
    let mut p = Protocol::new(Controlled(|_: Request, ctx: &CallContext| {
        std::thread::sleep(ctx.remaining() + Duration::from_millis(1));
        Ok(Response::ok(json!({"private":"not-returned"})))
    }));
    ready(&mut p);
    let r = p
        .handle_with_context(
            &serde_json::to_vec(&call("get_contacts", json!({}))).unwrap(),
            &CallContext::new(CancellationToken::default(), Duration::from_millis(10)),
        )
        .unwrap();
    assert_eq!(reply_text(&r), "Query timed out");
    assert!(!r.to_string().contains("not-returned"));
}

#[test]
fn failed_output_does_not_commit_poll_cursor() {
    let mut p = Protocol::new(|_| Ok(Response::ok(json!({"sessions":[session("a",1,1)]}))));
    ready(&mut p);
    let frame = serde_json::to_string(&call("get_new_messages", json!({}))).unwrap() + "\n";
    let mut output = Vec::new();
    assert!(p
        .serve(Cursor::new(frame.as_bytes()), &mut output, frame.len())
        .is_err());
    assert!(p.session_state.is_none());
    assert!(output.is_empty());
}

#[test]
fn external_thread_can_cancel_cooperative_dispatcher() {
    let token = CancellationToken::default();
    let context = CallContext::new(token.clone(), Duration::from_secs(2));
    let (started, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        token.cancel();
    });
    let mut p = Protocol::new(Controlled(move |_: Request, ctx: &CallContext| {
        started.send(()).unwrap();
        loop {
            ctx.check()?;
            std::thread::yield_now();
        }
    }));
    ready(&mut p);
    let response = p
        .handle_with_context(
            &serde_json::to_vec(&call("get_contacts", json!({}))).unwrap(),
            &context,
        )
        .unwrap();
    worker.join().unwrap();
    assert_eq!(reply_text(&response), "Query cancelled");
}
