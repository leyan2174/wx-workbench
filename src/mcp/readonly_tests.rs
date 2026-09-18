use super::*;

fn cases() -> Vec<Value> {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/mcp-protocol/routes.json"
    ))
    .unwrap();
    fixtures.into_iter().skip(15).collect()
}

#[test]
fn readonly_registration_and_defaults_are_narrow() {
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let tool = tools().into_iter().find(|tool| tool.name == name).unwrap();
        assert!(tool.read_only());
        assert!(!tool.open_world());
        assert!(!tool.destructive());
        assert_eq!(tool.input_schema["additionalProperties"], false);
        let mut args = json!({});
        for key in tool.input_schema["required"].as_array().unwrap() {
            args[key.as_str().unwrap()] = case["arguments"][key.as_str().unwrap()].clone();
        }
        let request = serde_json::to_value(route(name, &args).unwrap()).unwrap();
        assert_eq!(request["cmd"], case["ipc"]["cmd"]);
        if tool.input_schema["properties"].get("limit").is_some() {
            let default_limit = match name {
                "get_favorites" | "get_biz_articles" | "get_sns_notifications" => 50,
                _ => 20,
            };
            assert_eq!(request["limit"], default_limit);
        }
        if name == "get_sns_notifications" {
            assert_eq!(request["include_read"], false);
        }
        if name == "get_biz_articles" {
            assert_eq!(request["unread"], false);
        }
    }
}

#[test]
fn readonly_invalid_arguments_never_reach_dispatch() {
    let mut protocol = Protocol::new(|_: Request| -> Result<Response, DispatchError> {
        panic!("invalid arguments reached dispatcher")
    });
    ready(&mut protocol);
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let good = &case["arguments"];
        let tool = tools().into_iter().find(|tool| tool.name == name).unwrap();
        let mut bad = vec![json!(null), json!([])];
        for key in [
            "cmd",
            "db_dir",
            "keys_file",
            "output",
            "runtime_id",
            "offset",
            "operation",
            "debug_source",
        ] {
            let mut args = good.clone();
            args[key] = json!("untrusted");
            bad.push(args);
        }
        for key in tool.input_schema["required"].as_array().unwrap() {
            let mut args = good.clone();
            args.as_object_mut().unwrap().remove(key.as_str().unwrap());
            bad.push(args);
        }
        for (key, schema) in tool.input_schema["properties"].as_object().unwrap() {
            let values = match schema["type"].as_str().unwrap() {
                "integer" if key == "limit" => vec![
                    json!(0),
                    json!(501),
                    json!(-1),
                    json!(1.5),
                    json!("2"),
                    json!(null),
                    json!(u64::MAX),
                ],
                "integer" => vec![json!(1.5), json!("2"), json!(null), json!(u64::MAX)],
                "string" => vec![json!(false), json!(null), json!("x".repeat(4097))],
                "boolean" => vec![json!(0), json!("false"), json!(null)],
                "array" => vec![
                    json!("private"),
                    json!(["typo"]),
                    json!([null]),
                    json!(vec!["group"; 101]),
                ],
                other => panic!("unexpected schema type {other}"),
            };
            for value in values {
                let mut args = good.clone();
                args[key] = value;
                bad.push(args);
            }
        }
        if good.get("since").is_some() {
            let mut args = good.clone();
            args["since"] = json!(21);
            bad.push(args);
        }
        for args in bad {
            let reply = send(&mut protocol, call(name, args.clone())).unwrap();
            assert_eq!(reply["error"]["code"], -32602, "{name}: {args}");
        }
    }
    for (name, args) in [
        ("get_chat_members", json!({"chat_name":" "})),
        ("get_biz_articles", json!({"account":" "})),
        ("get_sns_feed", json!({"user":" "})),
        ("search_sns", json!({"keyword":" "})),
        ("get_favorites", json!({"fav_type":-1})),
        ("search_messages", json!({"keyword":"needle","chats":[" "]})),
    ] {
        assert_eq!(
            send(&mut protocol, call(name, args)).unwrap()["error"]["code"],
            -32602
        );
    }
}

#[test]
fn history_and_search_expose_all_supported_filter_and_metadata_fields() {
    let history = route("get_chat_history", &json!({"chat_name":"peer","limit":5,"offset":3,"since":10,"until":20,"msg_types":["text","image"],"oldest_first":true,"with_meta":true})).unwrap();
    assert_eq!(
        serde_json::to_value(history).unwrap(),
        json!({"cmd":"history","chat":"peer","limit":5,"offset":3,"since":10,"until":20,"msg_types":[1,3],"oldest_first":true,"with_meta":true})
    );
    let search = route("search_messages", &json!({"keyword":"needle","chats":["peer","group"],"limit":5,"offset":3,"since":10,"until":20,"msg_type":1,"with_meta":true})).unwrap();
    assert_eq!(
        serde_json::to_value(search).unwrap(),
        json!({"cmd":"search","keyword":"needle","chats":["peer","group"],"limit":8,"since":10,"until":20,"msg_type":1,"with_meta":true})
    );
    assert!(route(
        "search_messages",
        &json!({"keyword":"needle","offset":9999,"limit":2})
    )
    .is_err());
    assert!(route(
        "search_messages",
        &json!({"keyword":"needle","msg_types":["text"]})
    )
    .is_err());
    assert!(route(
        "search_messages",
        &json!({"keyword":"needle","oldest_first":true})
    )
    .is_err());
    assert_eq!(
        serde_json::to_value(route("get_new_messages", &json!({})).unwrap()).unwrap(),
        json!({"cmd":"sessions","limit":10001})
    );
    assert!(route("get_new_messages", &json!({"filter":["group"]})).is_err());
    for (name, args) in [
        ("get_chat_history", json!({"chat_name":"peer"})),
        ("search_messages", json!({"keyword":"needle"})),
        ("get_chat_stats", json!({"chat_name":"peer"})),
        ("get_unread_messages", json!({})),
    ] {
        for value in [json!(true), json!(false), json!(null)] {
            let mut args = args.clone();
            args["debug_source"] = value;
            assert!(route(name, &args).is_err());
        }
    }
}
