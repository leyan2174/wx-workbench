//! G1: encrypted synthetic accounts -> real daemon, CLI, MCP stdio, and Web HTTP.
//! Main owns compilation and the target directory; never resolve wx from PATH.
#![cfg(windows)]

#[path = "fixtures/g1-query/cases.rs"]
mod cases;
#[path = "../src/private_file.rs"]
#[allow(dead_code)]
mod private_file;
#[path = "fixtures/g1-query/runtime.rs"]
mod runtime;
#[path = "fixtures/g1-query/seed.rs"]
mod seed;
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;

use runtime::{Fixture, Web};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

// Also used by the read-only shared attachment fixture at compile time.
fn safe_failure(reply: Value, expected: &str) {
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
}

#[test]
fn g1_four_entrypoints_return_the_same_nonempty_account_bound_objects() {
    let mut a = Fixture::new("A", true);
    let mut b = Fixture::new("B", true);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.account.start();
    b.account.start();
    let wa = Web::start(&a);
    let wb = Web::start(&b);
    assert_ne!(wa.origin, wb.origin);
    assert_ne!(wa.token, wb.token);
    let mut ma = a.account.mcp();
    let mut mb = b.account.mcp();
    ma.ready();
    mb.ready();
    for (fixture, web, mcp) in [(&a, &wa, &mut ma), (&b, &wb, &mut mb)] {
        let discovery = mcp.rpc("tools/list", json!({}));
        let names: Vec<_> = discovery["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for case in cases::all(fixture.account.marker) {
            assert!(names.contains(&case.tool), "missing tool: {}", case.tool);
            // Compare the same warm-cache state, including cache_mode metadata.
            fixture.data(case.request.clone());
            let expected = fixture.data(case.request.clone());
            for (pointer, witness) in &case.witnesses {
                assert_eq!(
                    expected.pointer(pointer),
                    Some(witness),
                    "{} {pointer}: {expected}",
                    case.tool
                );
            }
            if case.arguments["with_meta"] == true {
                assert!(expected["meta"].is_object(), "{}: {expected}", case.tool);
                assert!(expected["meta"].get("shard_paths").is_none());
            }
            let cli_args: Vec<_> = case.cli.iter().map(String::as_str).collect();
            let output = fixture.cli(&cli_args);
            if case.tool == "get_favorites" || case.tool == "get_biz_articles" {
                assert!(
                    !output.stderr.trim().is_empty(),
                    "missing continuation/partial warning"
                );
            }
            let cli = output.data();
            let projected = case.cli_projection.map_or(&expected, |key| &expected[key]);
            assert_eq!(&cli, projected, "CLI {}", case.tool);
            assert_eq!(
                mcp.data(case.tool, case.arguments),
                expected,
                "MCP {}",
                case.tool
            );
            assert_eq!(
                cases::http_projection(case.tool, web.data(&case.http)),
                expected,
                "HTTP {}",
                case.tool
            );
        }
    }
    // Return to A after exercising B: no process-global account leakage.
    let expected = a.data(json!({"cmd":"tag_members","tag_name":"G1-A"}));
    assert_eq!(expected["members"][0]["display_name"], "PeerA");
    assert_eq!(
        ma.data("get_tag_members", json!({"tag_name":"G1-A"})),
        expected
    );
    assert_eq!(wa.data("/api/tag-members?name=G1-A"), expected);
    assert_eq!(a.cli(&["tag-members", "G1-A"]).data(), expected);
    // include_read is a query, not a mark-read operation.
    let unread = a.data(json!({"cmd":"sns_notifications","limit":20,"include_read":false}));
    assert_eq!(unread["total"], 1);
    assert_eq!(
        ma.data("get_sns_notifications", json!({"limit":20})),
        unread
    );
    assert_eq!(wa.data("/api/sns-notifications?limit=20"), unread);
    assert_eq!(
        a.cli(&["sns-notifications", "--limit", "20"]).data(),
        unread["notifications"]
    );
    ma.finish();
    mb.finish();
    drop(wa);
    drop(wb);
    a.account.stop();
    b.account.stop();
    assert_eq!(a.snapshot(), before_a, "queries modified account A sources");
    assert_eq!(b.snapshot(), before_b, "queries modified account B sources");
}

#[test]
fn g1_rejects_bad_arguments_and_ambiguous_targets_without_success_shaped_empty_results() {
    let mut fixture = Fixture::new("A", true);
    let before = fixture.snapshot();
    fixture.account.start();
    let web = Web::start(&fixture);
    let mut mcp = fixture.account.mcp();
    mcp.ready();
    for (tool, args, cli, path) in [
        (
            "get_voice_messages",
            json!({"chat_name":"peer","limit":0}),
            vec!["voice-messages", "peer", "--limit", "0"],
            "/api/voice-messages?chat=peer&limit=0",
        ),
        (
            "decode_record_item",
            json!({"chat_name":"peer","local_id":40,"create_time":4000,"item_index":-1}),
            vec!["decode-record-item", "peer", "40", "-1", "4000"],
            "/api/decode-record-item?chat=peer&local_id=40&create_time=4000&item_index=-1",
        ),
        (
            "get_chat_history",
            json!({"chat_name":"peer","msg_type":1,"msg_types":["image"]}),
            vec!["history", "peer", "--type", "text", "--types", "image"],
            "/api/history?chat=peer&msg_type=text&msg_types=image",
        ),
    ] {
        assert_eq!(mcp.call(tool, args)["error"]["code"], -32602, "{tool}");
        fixture.cli(&cli).failed();
        let (status, body) = web.get(path);
        assert_eq!(status, 400, "{body}");
        assert!(body["error"].is_string());
    }
    for path in [
        "/api/search?keyword=history&offset=1",
        "/api/stats?chat=peer&since=20&until=10",
        "/api/voice-messages?chat=peer&db_dir=outside",
        "/api/history?chat=peer&limit=1&limit=2",
        "/api/decode-refer?chat=peer&local_id=7",
    ] {
        assert_eq!(web.get(path).0, 400, "{path}");
    }
    for (tool, args, cli, path, request) in [
        (
            "get_tag_members",
            json!({"tag_name":"Ambiguous"}),
            vec!["tag-members", "Ambiguous"],
            "/api/tag-members?name=Ambiguous",
            json!({"cmd":"tag_members","tag_name":"Ambiguous"}),
        ),
        (
            "get_chat_stats",
            json!({"chat_name":"Same"}),
            vec!["stats", "Same"],
            "/api/stats?chat=Same",
            json!({"cmd":"stats","chat":"Same"}),
        ),
        (
            "decode_file_message",
            json!({"chat_name":"peer","local_id":30,"create_time":300}),
            vec!["decode-file-message", "peer", "30", "300"],
            "/api/decode-file-message?chat=peer&local_id=30&create_time=300",
            json!({"cmd":"decode_file_message","chat":"peer","local_id":30,"create_time":300}),
        ),
    ] {
        let raw = fixture.account.ipc(request).unwrap();
        assert!(
            raw["ok"] == false || raw["exit_code"] == 2,
            "ambiguity silently accepted: {raw}"
        );
        safe_failure(mcp.call(tool, args), "Business request refused");
        fixture.cli(&cli).failed();
        let (status, body) = web.get(path);
        assert_eq!(status, 409, "ambiguity must be an HTTP conflict: {body}");
        assert!(body["error"].is_string());
    }
    // Errors must not poison the running account/session.
    assert_eq!(
        mcp.data("get_contact_tags", json!({})),
        fixture.data(json!({"cmd":"contact_tags"}))
    );
    mcp.finish();
    drop(web);
    fixture.account.stop();
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn g1_missing_database_is_an_error_in_every_entrypoint() {
    let mut fixture = Fixture::new("A", false);
    assert!(!fixture
        .account
        .root()
        .join("db_storage/favorite/favorite.db")
        .exists());
    let before = fixture.snapshot();
    fixture.account.start();
    let web = Web::start(&fixture);
    let mut mcp = fixture.account.mcp();
    mcp.ready();
    let raw = fixture
        .account
        .ipc(json!({"cmd":"favorites","limit":1}))
        .unwrap();
    assert_eq!(raw["ok"], false, "{raw}");
    assert!(raw["error"].is_string());
    fixture.cli(&["favorites", "--limit", "1"]).failed();
    safe_failure(
        mcp.call("get_favorites", json!({"limit":1})),
        "Query failed",
    );
    let (status, body) = web.get("/api/favorites?limit=1");
    assert!(
        (400..600).contains(&status),
        "missing DB returned success: {body}"
    );
    assert!(body["error"].is_string());
    mcp.finish();
    drop(web);
    fixture.account.stop();
    assert_eq!(fixture.snapshot(), before);
}

#[test]
#[ignore = "browsermanual: explicit WX_G1_UI_FIXTURE_INFO required; main runs this separately with --ignored"]
fn g1_ui_fixture_browsermanual() {
    let info = PathBuf::from(
        std::env::var_os("WX_G1_UI_FIXTURE_INFO")
            .expect("browsermanual requires explicit WX_G1_UI_FIXTURE_INFO"),
    );
    assert!(info.is_absolute());
    let mut fixture = Fixture::new("A", true);
    let before = fixture.snapshot();
    fixture.account.start();
    let mut web = Web::start(&fixture);
    assert_eq!(web.data("/api/tags")["total_tags"], 3);
    let published = runtime::publish(&info, &web);
    println!("Synthetic browsermanual fixture ready: {}", info.display());
    let deadline = Instant::now() + Duration::from_secs(600);
    let stopped = loop {
        if published.1.is_file() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        web.assert_running();
        thread::sleep(Duration::from_millis(100));
    };
    drop(published);
    drop(web);
    fixture.account.stop();
    assert_eq!(
        fixture.snapshot(),
        before,
        "manual browsing modified source account"
    );
    assert!(
        stopped,
        "browsermanual timed out after ten minutes; children cleaned up"
    );
}
