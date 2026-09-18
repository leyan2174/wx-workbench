//! Published synthetic exports, read through the real task service and MCP process.
use super::super::personal_messages;
use super::{call, terminal, Fixture, Mcp, Web};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;

const EXPORT: &[&str] = &[
    "--tasks",
    "--task-kind",
    "export_all",
    "--task-allow-media-write",
    "--task-allow-artifact-read",
];

pub(super) fn refused(reply: &Value) {
    assert!(
        reply.get("error").is_some() || reply["result"]["isError"] == true,
        "unexpected successful response: {reply}"
    );
}

pub(super) fn read_artifact(mcp: &mut Mcp, task_id: &str, artifact: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = mcp.data(
            "read_task_artifact",
            json!({"id":task_id,"artifact_id":artifact["artifact_id"],"offset":offset}),
        );
        assert_eq!(chunk["task_id"], task_id);
        assert_eq!(chunk["artifact_id"], artifact["artifact_id"]);
        assert_eq!(chunk["offset"], offset);
        assert_eq!(chunk["sha256"], artifact["sha256"]);
        assert_eq!(chunk["size"], artifact["size"]);
        assert_eq!(chunk["encoding"], "base64");
        let block = base64::engine::general_purpose::STANDARD
            .decode(chunk["data_base64"].as_str().unwrap())
            .unwrap();
        assert!(block.len() <= 1048576);
        assert_eq!(chunk["bytes_read"], block.len() as u64);
        bytes.extend_from_slice(&block);
        if chunk["eof"] == true {
            break;
        }
        assert!(!block.is_empty(), "non-terminal read must advance");
        let next = chunk["next_offset"].as_u64().unwrap();
        assert_eq!(next, offset + block.len() as u64);
        offset = next;
    }
    assert_eq!(bytes.len() as u64, artifact["size"].as_u64().unwrap());
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), artifact["sha256"]);
    bytes
}

#[test]
fn published_exports_survive_mcp_disconnect_and_are_account_bound() {
    let mut fixture = Fixture::new();
    let account = fixture.account("artifact-owner", true);
    let other = fixture.account("artifact-other", true);
    let source = personal_messages(&fixture, &account, 16384);
    let before = fs::read(&source).unwrap();
    let mut mcp = Mcp::start(&fixture, &account, EXPORT);
    let id = "e1".repeat(32);
    let submission = json!({"idempotency_key":id,"kind":"export_all",
        "options":{"users":["task-peer"],"formats":["json","csv","html"],"include_images":false}});
    let submitted = mcp.data("submit_task", submission.clone());
    assert_eq!(submitted["id"], id);
    mcp.close();

    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(task["result"]["scope"], "chat_directory");
    assert_eq!(task["result"]["finalized"], true);
    assert_eq!(task["result"]["outcome"], "success");
    assert_eq!(task["result"]["exported_chats"], 1);
    assert_eq!(task["result"]["messages"], 16384);

    let mut mcp = Mcp::start(&fixture, &account, EXPORT);
    assert_eq!(mcp.data("submit_task", submission), task);
    let page = mcp.data(
        "list_task_artifacts",
        json!({"id":id,"offset":0,"limit":100}),
    );
    assert_eq!(page["task_id"], id);
    assert_eq!(page["complete"], true);
    assert!(page["next_offset"].is_null());
    let items = page["items"].as_array().unwrap();
    assert!(items.len() >= 3, "{page}");
    assert_eq!(page["total"], items.len() as u64);
    assert_eq!(task["result"]["artifact_count"], items.len() as u64);
    assert_eq!(
        call(
            &fixture,
            &account,
            &["tasks", "artifacts", &id, "--limit", "100"]
        ),
        page
    );
    let artifact = items
        .iter()
        .find(|item| item["role"] == "chat_document" && item["media_type"] == "application/json")
        .expect("published JSON chat document");
    assert!(artifact["size"].as_u64().unwrap() > 1048576);
    let cli_chunk = call(
        &fixture,
        &account,
        &[
            "tasks",
            "read-artifact",
            &id,
            artifact["artifact_id"].as_str().unwrap(),
            "--max-bytes",
            "17",
        ],
    );
    assert_eq!(cli_chunk["bytes_read"], 17);
    assert_eq!(cli_chunk["sha256"], artifact["sha256"]);
    let bytes = read_artifact(&mut mcp, &id, artifact);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(cli_chunk["data_base64"].as_str().unwrap())
            .unwrap(),
        bytes[..17]
    );
    assert!(String::from_utf8(bytes.clone())
        .unwrap()
        .contains("synthetic message"));
    let web = Web::start(&fixture, &account);
    let listed = web.request(
        reqwest::Method::GET,
        &format!("/api/tasks/{id}/artifacts?offset=0&limit=100"),
        None,
        None,
    );
    assert_eq!(listed.status(), reqwest::StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&listed.text().unwrap()).unwrap(),
        page
    );
    let download_path = format!(
        "/api/tasks/{id}/artifacts/{}/download",
        artifact["artifact_id"].as_str().unwrap()
    );
    let response = web.request(reqwest::Method::GET, &download_path, None, None);
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(response.bytes().unwrap().as_ref(), bytes.as_slice());
    let ticket_response = web.request(
        reqwest::Method::POST,
        &format!(
            "/api/tasks/{id}/artifacts/{}/ticket",
            artifact["artifact_id"].as_str().unwrap()
        ),
        None,
        None,
    );
    assert_eq!(ticket_response.status(), reqwest::StatusCode::OK);
    assert!(!ticket_response.headers().contains_key("set-cookie"));
    let ticket: Value = serde_json::from_str(&ticket_response.text().unwrap()).unwrap();
    let url = ticket["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("{download_path}?")));
    assert!(!url.contains(&web.token));
    let ticket_download = web.http.get(format!("{}{url}", web.origin)).send().unwrap();
    assert_eq!(ticket_download.status(), reqwest::StatusCode::OK);
    assert_eq!(ticket_download.bytes().unwrap().as_ref(), bytes.as_slice());
    let query = url.split_once('?').unwrap().1;
    for path in [
        format!("/api/tasks/{id}?{query}"),
        format!(
            "/api/tasks/{id}/artifacts/{}/download?{query}",
            "f1".repeat(32)
        ),
        format!("{download_path}?{query}&expires=0"),
    ] {
        assert_eq!(
            web.http
                .get(format!("{}{path}", web.origin))
                .send()
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
    }
    let html = items
        .iter()
        .find(|item| item["media_type"] == "text/html")
        .unwrap();
    let html_response = web.request(
        reqwest::Method::GET,
        &format!(
            "/api/tasks/{id}/artifacts/{}/download",
            html["artifact_id"].as_str().unwrap()
        ),
        None,
        None,
    );
    assert_eq!(html_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        html_response.headers()["content-type"],
        "application/octet-stream"
    );
    assert!(html_response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert!(html_response.text().unwrap().contains("synthetic message"));
    let ranged = web
        .http
        .get(format!("{}{download_path}", web.origin))
        .header("x-wx-token", &web.token)
        .header("origin", &web.origin)
        .header("range", "bytes=0-9")
        .send()
        .unwrap();
    assert_eq!(ranged.status(), reqwest::StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        web.http
            .get(format!("{}{download_path}", web.origin))
            .send()
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert!(!page
        .to_string()
        .contains(&account.to_string_lossy().to_string()));
    for item in items {
        let name = item["name"].as_str().unwrap();
        assert!(!name.contains(['/', '\\']));
        assert!(item.get("path").is_none());
    }
    let first = mcp.data("list_task_artifacts", json!({"id":id,"offset":0,"limit":1}));
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["next_offset"], 1);
    for arguments in [
        json!({"id":id,"artifact_id":"../config.json"}),
        json!({"id":id,"artifact_id":artifact["artifact_id"],"path":"config.json"}),
        json!({"id":id,"artifact_id":artifact["artifact_id"],"max_bytes":1048577}),
    ] {
        refused(&mcp.tool("read_task_artifact", arguments));
    }
    let mut other_mcp = Mcp::start(&fixture, &other, EXPORT);
    refused(&other_mcp.tool("list_task_artifacts", json!({"id":id})));
    refused(&other_mcp.tool(
        "read_task_artifact",
        json!({"id":id,"artifact_id":artifact["artifact_id"]}),
    ));
    let mut manager = Mcp::start(&fixture, &account, &["--tasks"]);
    assert_eq!(
        manager.data("get_task", json!({"id":id}))["status"],
        "succeeded"
    );
    let discovery = manager.request("tools/list", json!({}));
    assert!(!discovery["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| {
            matches!(
                tool["name"].as_str(),
                Some("list_task_artifacts" | "read_task_artifact")
            )
        }));
    refused(&manager.tool(
        "read_task_artifact",
        json!({"id":id,"artifact_id":artifact["artifact_id"]}),
    ));
    assert_eq!(fs::read(source).unwrap(), before);
    assert_eq!(
        call(&fixture, &account, &["tasks", "list"])["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn export_dry_run_and_media_budgets_preserve_host_authorization() {
    let mut fixture = Fixture::new();
    let account = fixture.account("artifact-dry-run", true);
    let source = personal_messages(&fixture, &account, 3);
    let before = fs::read(&source).unwrap();
    let id = "e2".repeat(32);
    let submission = json!({"idempotency_key":id,"kind":"export_all",
        "options":{"users":["task-peer"],"formats":["json"],"include_images":false,"dry_run":true}});
    let mut unauthorized = Mcp::start(
        &fixture,
        &account,
        &["--tasks", "--task-kind", "export_all"],
    );
    refused(&unauthorized.tool("submit_task", submission.clone()));
    unauthorized.close();
    let mut mcp = Mcp::start(&fixture, &account, EXPORT);
    for options in [
        json!({"include_images":false,"max_media_bytes":1024}),
        json!({"include_images":true,"max_media_bytes":0}),
        json!({"include_images":true,"max_media_bytes":2048,"max_total_media_bytes":1024}),
        json!({"include_images":true,"max_media_bytes":524288001}),
        json!({"include_images":true,"max_total_media_bytes":17179869185u64}),
    ] {
        refused(&mcp.tool(
            "submit_task",
            json!({"idempotency_key":id,"kind":"export_all","options":options}),
        ));
    }
    assert_eq!(mcp.data("submit_task", submission)["id"], id);
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(task["result"]["dry_run"], true);
    assert_eq!(task["result"]["planned_chats"], 1);
    assert_eq!(task["result"]["exported_chats"], 0);
    assert_eq!(task["result"]["artifact_count"], 0);
    let page = mcp.data("list_task_artifacts", json!({"id":id}));
    assert_eq!(page["items"], json!([]));
    assert_eq!(page["total"], 0);
    assert_eq!(fs::read(source).unwrap(), before);
    let tasks = call(&fixture, &account, &["tasks", "list"]);
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn later_step_failure_preserves_published_chat_artifacts_without_claiming_task_success() {
    let mut fixture = Fixture::new();
    let account = fixture.account("artifact-partial", true);
    personal_messages(&fixture, &account, 3);
    // No SNS database or key is installed; the later SNS step must fail locally.
    let mut mcp = Mcp::start(
        &fixture,
        &account,
        &[
            "--tasks",
            "--task-kind",
            "export_all,sns_decrypt",
            "--task-allow-media-write",
            "--task-allow-artifact-read",
        ],
    );
    let id = "e3".repeat(32);
    mcp.data("submit_task", json!({"idempotency_key":id,"kind":"export_all",
        "options":{"users":["task-peer"],"formats":["json"],"include_images":false,"include_sns":true}}));
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "failed", "{task}");
    assert_eq!(task["result"]["scope"], "chat_directory");
    assert_eq!(task["result"]["outcome"], "success");
    assert_eq!(task["result"]["exported_chats"], 1);
    let page = mcp.data("list_task_artifacts", json!({"id":id}));
    let artifact = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "chat_document")
        .expect("published document retained after a later step failed");
    let bytes = read_artifact(&mut mcp, &id, artifact);
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("synthetic message"));
    assert_eq!(
        mcp.data("cancel_task", json!({"id":id}))["status"],
        "failed"
    );
}
