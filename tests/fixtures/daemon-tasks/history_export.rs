//! Single-conversation exports through real, account-bound daemon workers.
use super::super::personal_messages;
use super::artifacts::{read_artifact, refused};
use super::{call, terminal, Fixture, Mcp, Web};
use serde_json::{json, Value};
use std::fs;

const HOST: &[&str] = &[
    "--tasks",
    "--task-kind",
    "export_history",
    "--task-allow-artifact-read",
];

fn submission(id: &str, format: &str, limit: usize) -> Value {
    json!({"idempotency_key":id,"kind":"export_history","options":{
        "history_export":{"chat":"task-peer","format":format,"limit":limit}
    }})
}

#[test]
fn history_task_four_formats_match_foreground_and_share_artifacts() {
    let mut fixture = Fixture::new();
    let account = fixture.account("history-export", true);
    let source = personal_messages(&fixture, &account, 12);
    let before = fs::read(&source).unwrap();
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let web = Web::start(&fixture, &account);

    for (index, format) in ["markdown", "txt", "json", "yaml"].into_iter().enumerate() {
        let id = format!("{:064x}", 0x3100 + index);
        let request = submission(&id, format, 7);
        let queued = mcp.data("submit_task", request.clone());
        assert_eq!(queued["id"], id);
        let task = terminal(&fixture, &account, &id);
        assert_eq!(task["status"], "succeeded", "{task}");
        assert_eq!(task["result"]["scope"], "chat_history");
        assert_eq!(task["result"]["outcome"], "success");
        assert_eq!(task["result"]["query"]["username"], "task-peer");
        assert_eq!(task["result"]["query"]["messages"], 7);
        assert!(task["result"].get("exported_chats").is_none());
        assert_eq!(mcp.data("submit_task", request), task);

        let page = mcp.data("list_task_artifacts", json!({"id":id}));
        assert_eq!(page["scope"], "chat_history");
        assert_eq!(page["complete"], true);
        assert_eq!(page["total"], 1);
        let artifact = &page["items"][0];
        let exported = read_artifact(&mut mcp, &id, artifact);
        assert_eq!(call(&fixture, &account, &["tasks", "artifacts", &id]), page);

        let expected = fixture.root.join(format!("foreground-{format}.out"));
        crate::success(fixture.run(
            &account,
            &[
                "export",
                "task-peer",
                "--limit",
                "7",
                "--format",
                format,
                "--output",
                expected.to_str().unwrap(),
            ],
        ));
        assert_eq!(exported, fs::read(expected).unwrap(), "format {format}");
        let response = web.request(
            reqwest::Method::GET,
            &format!(
                "/api/tasks/{id}/artifacts/{}/download",
                artifact["artifact_id"].as_str().unwrap()
            ),
            None,
            None,
        );
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.bytes().unwrap().as_ref(), exported);
    }
    let cli_task = call(
        &fixture,
        &account,
        &[
            "tasks",
            "submit",
            "export_history",
            "--chat",
            "task-peer",
            "--format",
            "txt",
            "--limit",
            "2",
            "--wait",
        ],
    );
    assert_eq!(cli_task["status"], "succeeded", "{cli_task}");
    assert_eq!(cli_task["result"]["query"]["messages"], 2);
    let id = cli_task["id"].as_str().unwrap();
    assert_eq!(mcp.data("get_task", json!({"id":id})), cli_task);
    let page = mcp.data("list_task_artifacts", json!({"id":id}));
    assert!(!read_artifact(&mut mcp, id, &page["items"][0]).is_empty());
    assert_eq!(fs::read(source).unwrap(), before);
}

#[test]
fn history_task_preserves_date_end_and_large_limit_after_disconnect() {
    use chrono::TimeZone;
    let mut fixture = Fixture::new();
    let account = fixture.account("history-window", true);
    let source = personal_messages(&fixture, &account, 10001);
    let plain = account.join("messages-plain.db");
    let db = rusqlite::Connection::open(&plain).unwrap();
    let base = chrono::Local
        .with_ymd_and_hms(2026, 9, 18, 12, 0, 0)
        .single()
        .unwrap()
        .timestamp();
    let table = format!("Msg_{:x}", md5::compute("task-peer"));
    db.execute(
        &format!("UPDATE [{table}] SET create_time=?1+local_id"),
        [base],
    )
    .unwrap();
    drop(db);
    crate::encrypt_fixture(&plain, &source);
    let before = fs::read(&source).unwrap();
    let web = Web::start(&fixture, &account);
    let id = "32".repeat(32);
    let request = json!({"kind":"export_history","options":{"history_export":{
        "chat":"task-peer","format":"json","limit":10001,
        "since":"2026-09-18","until":"2026-09-18"
    }}});
    let reply = web.request(
        reqwest::Method::POST,
        "/api/tasks",
        Some(&id),
        Some(request),
    );
    assert_eq!(reply.status(), reqwest::StatusCode::ACCEPTED);
    drop(web);
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(task["result"]["query"]["messages"], 10001);
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let page = mcp.data("list_task_artifacts", json!({"id":id}));
    let bytes = read_artifact(&mut mcp, &id, &page["items"][0]);
    let data: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["messages"].as_array().unwrap().len(), 10001);

    let exact_id = "33".repeat(32);
    let mut exact = submission(&exact_id, "json", 10001);
    exact["options"]["history_export"]["since"] = json!("2026-09-18");
    exact["options"]["history_export"]["until"] = json!("2026-09-18 12:00:03");
    mcp.data("submit_task", exact);
    mcp.close();
    let exact = terminal(&fixture, &account, &exact_id);
    assert_eq!(exact["status"], "succeeded", "{exact}");
    assert_eq!(exact["result"]["query"]["messages"], 3);
    assert_eq!(fs::read(source).unwrap(), before);
}

#[test]
fn history_task_rejects_unauthorized_and_invalid_requests_without_enqueuing() {
    let mut fixture = Fixture::new();
    let account = fixture.account("history-rejections", true);
    personal_messages(&fixture, &account, 3);
    let id = "34".repeat(32);
    let valid = submission(&id, "json", 3);
    let mut manager = Mcp::start(
        &fixture,
        &account,
        &["--tasks", "--task-allow-artifact-read"],
    );
    refused(&manager.tool("submit_task", valid.clone()));
    manager.close();
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    for (field, value) in [
        ("limit", json!(0)),
        ("format", json!("csv")),
        ("until", json!("not-a-date")),
        ("output", json!("escape.txt")),
        ("chat", json!("")),
    ] {
        let mut invalid = valid.clone();
        invalid["options"]["history_export"][field] = value;
        refused(&mcp.tool("submit_task", invalid));
    }
    let mut reverse = valid.clone();
    reverse["options"]["history_export"]["since"] = json!("2026-09-19");
    reverse["options"]["history_export"]["until"] = json!("2026-09-18");
    refused(&mcp.tool("submit_task", reverse));
    mcp.data("submit_task", valid);
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    let tasks = call(&fixture, &account, &["tasks", "list"]);
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn missing_history_conversation_is_failure_not_an_empty_export() {
    let mut fixture = Fixture::new();
    let account = fixture.account("history-missing", true);
    personal_messages(&fixture, &account, 3);
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let id = "35".repeat(32);
    let mut request = submission(&id, "json", 3);
    request["options"]["history_export"]["chat"] = json!("no-such-synthetic-contact");
    mcp.data("submit_task", request);
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "failed", "{task}");
    assert_ne!(task["result"]["outcome"], "success");
    assert!(task["result"]["query"].is_null());
    let reply = mcp.tool("list_task_artifacts", json!({"id":id}));
    if reply.get("error").is_none() && reply["result"]["isError"] != true {
        assert_eq!(reply["result"]["structuredContent"]["total"], 0, "{reply}");
    }
}
