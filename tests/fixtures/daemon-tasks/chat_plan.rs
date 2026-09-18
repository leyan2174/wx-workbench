//! Account-bound plan generation, immutable review and fresh apply through real workers.
use super::super::personal_messages;
use super::artifacts::{read_artifact, refused};
use super::{call, terminal, Fixture, Mcp, Web};
use base64::Engine;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const HOST: &[&str] = &[
    "--tasks",
    "--task-kind",
    "chat_plan",
    "--task-kind",
    "chat_plan_review",
    "--task-kind",
    "chat_plan_apply",
    "--task-allow-artifact-read",
];

fn seed(fixture: &mut Fixture, name: &str) -> (PathBuf, Vec<(PathBuf, Vec<u8>)>) {
    let account = fixture.account(name, true);
    let source = personal_messages(fixture, &account, 3);
    let plain = account.join("messages-plain.db");
    let db = rusqlite::Connection::open(&plain).unwrap();
    let table = format!("Msg_{:x}", md5::compute("task-other"));
    db.execute_batch(&format!(
        "INSERT INTO Name2Id(user_name) VALUES('task-other');
         CREATE TABLE [{table}] (local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER);
         INSERT INTO [{table}] VALUES(1,1,1,NULL,'other synthetic one',0),(2,1,2,NULL,'other synthetic two',0);"
    )).unwrap();
    let orphan_table = format!("Msg_{:x}", md5::compute("task-orphan"));
    db.execute_batch(&format!("INSERT INTO Name2Id(user_name) VALUES('task-orphan'); CREATE TABLE [{orphan_table}] (local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{orphan_table}] VALUES(1,1,1,NULL,'orphan synthetic',0);")).unwrap();
    drop(db);
    crate::encrypt_fixture(&plain, &source);
    let contacts = rusqlite::Connection::open(account.join("fixture.db")).unwrap();
    contacts
        .execute_batch(
            "INSERT INTO contact VALUES('task-peer','Same synthetic name','',0);
         INSERT INTO contact VALUES('task-other','Same synthetic name','',0);",
        )
        .unwrap();
    drop(contacts);
    let contact_source = account.join("db_storage/contact/contact.db");
    crate::encrypt_fixture(&account.join("fixture.db"), &contact_source);
    let session_plain = account.join("sessions-plain.db");
    fs::copy(account.join("fixture.db"), &session_plain).unwrap();
    let sessions = rusqlite::Connection::open(&session_plain).unwrap();
    sessions.execute_batch("CREATE TABLE SessionTable(username TEXT,type INTEGER,last_timestamp INTEGER); INSERT INTO SessionTable VALUES('task-peer',0,3),('task-other',0,2);").unwrap();
    drop(sessions);
    let session_source = account.join("db_storage/session/session.db");
    fs::create_dir_all(session_source.parent().unwrap()).unwrap();
    crate::encrypt_fixture(&session_plain, &session_source);
    let keys = json!({"contact/contact.db":"11".repeat(32),"message/message_0.db":"11".repeat(32),"session/session.db":"11".repeat(32)});
    fixture.seed_keys(&account, &keys);
    let before = [source, contact_source, session_source]
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    (account, before)
}

fn submit(mcp: &mut Mcp, id: &str, kind: &str, request: Value) -> Value {
    let mut options = serde_json::Map::new();
    options.insert(kind.into(), request);
    mcp.data(
        "submit_task",
        json!({"idempotency_key":id,"kind":kind,"options":options}),
    )
}

fn finished(fixture: &Fixture, account: &Path, id: &str) -> Value {
    let task = terminal(fixture, account, id);
    assert_eq!(task["status"], "succeeded", "{task}");
    task
}

fn reference(task: &Value) -> Value {
    let reference = task["result"]["published_plan_ref"].clone();
    for key in ["task_id", "artifact_id", "sha256"] {
        assert_eq!(reference[key].as_str().unwrap().len(), 64, "{task}");
    }
    reference
}

fn read(mcp: &mut Mcp, reference: &Value, mode: &str) -> Value {
    mcp.data(
        "read_chat_plan",
        json!({"plan_ref":reference,"plan_mode":mode,"offset":0,"limit":100}),
    )
}

#[test]
fn generated_plan_review_and_apply_share_the_existing_selection_rules() {
    let mut fixture = Fixture::new();
    let (account, mut before) = seed(&mut fixture, "plan-workflow");
    let cached = account.join("decrypted/message/message_0.db");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    fs::copy(account.join("messages-plain.db"), &cached).unwrap();
    let poison = rusqlite::Connection::open(&cached).unwrap();
    let table = format!("Msg_{:x}", md5::compute("task-peer"));
    poison
        .execute_batch(&format!(
        "WITH RECURSIVE numbers(n) AS (SELECT 100 UNION ALL SELECT n+1 FROM numbers WHERE n<199)
         INSERT INTO [{table}] SELECT n,1,n,NULL,'foreign stale cache',0 FROM numbers;"
    ))
        .unwrap();
    drop(poison);
    before.push((cached.clone(), fs::read(&cached).unwrap()));
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let id = "41".repeat(32);
    let request =
        json!({"users":["task-peer","task-other","task-peer"],"size_mode":"estimate","threads":1});
    submit(&mut mcp, &id, "chat_plan", request.clone());
    let task = finished(&fixture, &account, &id);
    let original = reference(&task);
    let page = read(&mut mcp, &original, "blacklist");
    assert_eq!(page["total"], 2);
    assert_eq!(page["selected_count"], 2);
    assert_eq!(page["source_kind"], "runtime_snapshot");
    assert_eq!(read(&mut mcp, &original, "whitelist")["selected_count"], 0);
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row["username"] == "task-peer")
            .unwrap()["message_count"],
        3,
        "{page}"
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["username"] == "task-other")
            .unwrap()["message_count"],
        2
    );
    assert!(rows.iter().all(|row| row["export"] == ""));
    assert!(rows
        .iter()
        .all(|row| !row["size_status"].as_str().unwrap().is_empty()));
    assert_eq!(submit(&mut mcp, &id, "chat_plan", request), task);

    let review_id = "42".repeat(32);
    submit(
        &mut mcp,
        &review_id,
        "chat_plan_review",
        json!({"plan_ref":original,"changes":[
            {"username":"task-peer","export":"1"},{"username":"task-other","export":"0"}
        ]}),
    );
    let reviewed = reference(&finished(&fixture, &account, &review_id));
    assert_ne!(reviewed, original);
    assert_eq!(read(&mut mcp, &original, "blacklist")["selected_count"], 2);
    assert_eq!(read(&mut mcp, &reviewed, "blacklist")["selected_count"], 1);
    assert_eq!(read(&mut mcp, &reviewed, "whitelist")["selected_count"], 1);
    let apply_id = "43".repeat(32);
    submit(
        &mut mcp,
        &apply_id,
        "chat_plan_apply",
        json!({"plan_ref":reviewed,"plan_mode":"whitelist"}),
    );
    mcp.close();
    let applied = finished(&fixture, &account, &apply_id);
    assert_eq!(applied["result"]["scope"], "chat_plan_apply");
    assert_eq!(applied["result"]["selected_count"], 1);
    assert_eq!(applied["result"]["published_count"], 1);
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let artifacts = mcp.data("list_task_artifacts", json!({"id":apply_id}));
    assert_eq!(artifacts["total"], 1);
    let artifact = &artifacts["items"][0];
    let bytes = read_artifact(&mut mcp, &apply_id, artifact);
    let exported: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported["username"], "task-peer");
    let messages = exported["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert!(messages
        .iter()
        .all(|message| message["content"] == "synthetic message ".repeat(4)));
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(message["local_id"], index + 1);
        assert_eq!(message["timestamp"], index + 1);
    }
    let artifact_id = artifact["artifact_id"].as_str().unwrap();
    let cli = call(
        &fixture,
        &account,
        &["tasks", "read-artifact", &apply_id, artifact_id],
    );
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(cli["data_base64"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    let web = Web::start(&fixture, &account);
    let plan_response = web.request(
        reqwest::Method::GET,
        &format!(
            "/api/tasks/{}/artifacts/{}/plan?sha256={}&plan_mode=blacklist&offset=0&limit=100",
            original["task_id"].as_str().unwrap(),
            original["artifact_id"].as_str().unwrap(),
            original["sha256"].as_str().unwrap()
        ),
        None,
        None,
    );
    assert_eq!(plan_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&plan_response.bytes().unwrap()).unwrap(),
        page
    );
    let cli_plan = call(
        &fixture,
        &account,
        &[
            "tasks",
            "read-plan",
            "--plan-ref",
            &original.to_string(),
            "--limit",
            "100",
        ],
    );
    assert_eq!(cli_plan, page);
    let response = web.request(
        reqwest::Method::GET,
        &format!("/api/tasks/{apply_id}/artifacts/{artifact_id}/download"),
        None,
        None,
    );
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.bytes().unwrap().as_ref(), bytes);
    assert_eq!(
        call(&fixture, &account, &["tasks", "get", &apply_id]),
        applied
    );

    let empty_id = "44".repeat(32);
    submit(
        &mut mcp,
        &empty_id,
        "chat_plan_apply",
        json!({"plan_ref":original,"plan_mode":"whitelist"}),
    );
    let empty = finished(&fixture, &account, &empty_id);
    assert_eq!(empty["result"]["selected_count"], 0);
    assert_eq!(empty["result"]["published_count"], 0);
    assert_eq!(
        mcp.data("list_task_artifacts", json!({"id":empty_id}))["total"],
        0
    );
    let empty_plan_id = "4a".repeat(32);
    submit(&mut mcp, &empty_plan_id, "chat_plan", json!({"users":[]}));
    let empty_plan = reference(&finished(&fixture, &account, &empty_plan_id));
    let empty_page = read(&mut mcp, &empty_plan, "blacklist");
    assert_eq!(empty_page["total"], 0);
    assert_eq!(empty_page["selected_count"], 0);
    assert!(empty_page["rows"].as_array().unwrap().is_empty());
    let range_id = "4d".repeat(32);
    submit(
        &mut mcp,
        &range_id,
        "chat_plan",
        json!({"users":["task-peer"],"start":"2","end":"3"}),
    );
    let range_ref = reference(&finished(&fixture, &account, &range_id));
    let range_page = read(&mut mcp, &range_ref, "blacklist");
    assert_eq!(range_page["start_ts"], 2);
    assert_eq!(range_page["end_ts"], 3);
    assert_eq!(range_page["rows"][0]["message_count"], 2);
    let range_apply_id = "4e".repeat(32);
    submit(
        &mut mcp,
        &range_apply_id,
        "chat_plan_apply",
        json!({"plan_ref":range_ref}),
    );
    finished(&fixture, &account, &range_apply_id);
    let range_artifacts = mcp.data("list_task_artifacts", json!({"id":range_apply_id}));
    let range_export: Value = serde_json::from_slice(&read_artifact(
        &mut mcp,
        &range_apply_id,
        &range_artifacts["items"][0],
    ))
    .unwrap();
    assert_eq!(range_export["username"], "task-peer");
    assert_eq!(range_export["messages"].as_array().unwrap().len(), 2);
    for (index, message) in range_export["messages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(message["local_id"], index + 2);
        assert_eq!(message["timestamp"], index + 2);
    }
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn plan_refs_are_account_bound_and_scan_permission_is_not_a_model_option() {
    let mut fixture = Fixture::new();
    let (account, before) = seed(&mut fixture, "plan-auth");
    let (other, _) = seed(&mut fixture, "plan-other");
    let mut mcp = Mcp::start(&fixture, &account, HOST);
    let id = "45".repeat(32);
    refused(&mcp.tool(
        "submit_task",
        json!({"idempotency_key":id,"kind":"chat_plan","options":{
            "chat_plan":{"users":["task-peer"],"size_mode":"scan"}
        }}),
    ));
    refused(&mcp.tool(
        "submit_task",
        json!({"idempotency_key":id,"kind":"chat_plan","options":{
            "chat_plan":{"users":["task-peer"],"allow_plan_scan":true}
        }}),
    ));
    submit(&mut mcp, &id, "chat_plan", json!({}));
    let plan = reference(&finished(&fixture, &account, &id));
    let page = read(&mut mcp, &plan, "blacklist");
    assert_eq!(page["total"], 2);
    assert!(page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["username"] != "task-orphan"));
    refused(&mcp.tool("submit_task", json!({"idempotency_key":"4f".repeat(32),"kind":"chat_plan","options":{"chat_plan":{"users":["task-orphan"]}}})));
    let mut forged = plan.clone();
    forged["sha256"] = json!("ff".repeat(32));
    refused(&mcp.tool("read_chat_plan", json!({"plan_ref":forged})));
    let mut foreign = Mcp::start(&fixture, &other, HOST);
    refused(&foreign.tool("read_chat_plan", json!({"plan_ref":plan})));
    for (kind, suffix) in [("chat_plan_review", "4b"), ("chat_plan_apply", "4c")] {
        for (client, reference) in [(&mut mcp, &forged), (&mut foreign, &plan)] {
            let mut options = serde_json::Map::new();
            let request = if kind == "chat_plan_review" {
                json!({"plan_ref":reference,"changes":[]})
            } else {
                json!({"plan_ref":reference})
            };
            options.insert(kind.into(), request);
            refused(&client.tool(
                "submit_task",
                json!({"idempotency_key":suffix.repeat(32),"kind":kind,"options":options}),
            ));
        }
    }
    assert!(call(&fixture, &other, &["tasks", "list"])["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
    let mut no_read = Mcp::start(
        &fixture,
        &account,
        &["--tasks", "--task-kind", "chat_plan_review"],
    );
    refused(&no_read.tool("read_chat_plan", json!({"plan_ref":plan})));
    refused(&no_read.tool(
        "submit_task",
        json!({"idempotency_key":"46".repeat(32),"kind":"chat_plan_review",
        "options":{"chat_plan_review":{"plan_ref":plan,"changes":[]}}}),
    ));
    assert_eq!(
        call(&fixture, &account, &["tasks", "list"])["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn scan_permission_is_local_to_the_host_but_task_management_is_shared() {
    let mut fixture = Fixture::new();
    let (account, before) = seed(&mut fixture, "plan-hosts");
    fs::create_dir(account.join("msg")).unwrap();
    let web = Web::start(&fixture, &account);
    let request = json!({"kind":"chat_plan","options":{"chat_plan":{
        "users":["task-peer"],"size_mode":"scan","threads":2
    }}});
    let id = "47".repeat(32);
    let denied = web.request(
        reqwest::Method::POST,
        "/api/tasks",
        Some(&id),
        Some(request.clone()),
    );
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    let mut flags = HOST.to_vec();
    flags.push("--task-allow-plan-scan");
    let mut allowed_mcp = Mcp::start(&fixture, &account, &flags);
    submit(
        &mut allowed_mcp,
        &id,
        "chat_plan",
        request["options"]["chat_plan"].clone(),
    );
    let task = finished(&fixture, &account, &id);
    let observed = web
        .request(
            reqwest::Method::GET,
            &format!("/api/tasks/{id}"),
            None,
            None,
        )
        .bytes()
        .unwrap();
    let observed: Value = serde_json::from_slice(&observed).unwrap();
    assert_eq!(observed, task);
    let retry = web.request(
        reqwest::Method::POST,
        "/api/tasks",
        Some(&id),
        Some(request.clone()),
    );
    assert!(retry.status().is_success());
    assert_eq!(
        serde_json::from_slice::<Value>(&retry.bytes().unwrap()).unwrap(),
        task
    );
    let mut changed = request.clone();
    changed["options"]["chat_plan"]["threads"] = json!(1);
    assert_eq!(
        web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&id),
            Some(changed)
        )
        .status(),
        reqwest::StatusCode::CONFLICT
    );
    let cancelled = web.request(
        reqwest::Method::POST,
        &format!("/api/tasks/{id}/cancel"),
        None,
        Some(json!({})),
    );
    assert!(cancelled.status().is_success());

    drop(web);
    let allowed_web = Web::start_with_flags(&fixture, &account, &["--task-allow-plan-scan"]);
    let other_id = "48".repeat(32);
    let accepted = allowed_web.request(
        reqwest::Method::POST,
        "/api/tasks",
        Some(&other_id),
        Some(request.clone()),
    );
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
    let other_task = finished(&fixture, &account, &other_id);
    drop(allowed_web);
    let web = Web::start(&fixture, &account);
    let mut ordinary_mcp = Mcp::start(&fixture, &account, HOST);
    assert_eq!(
        ordinary_mcp.data("get_task", json!({"id":other_id})),
        other_task
    );
    assert_eq!(
        ordinary_mcp.data("cancel_task", json!({"id":other_id}))["status"],
        "succeeded"
    );
    let denied_id = "49".repeat(32);
    let mut model_request = request.clone();
    model_request["idempotency_key"] = json!(denied_id);
    refused(&ordinary_mcp.tool("submit_task", model_request));
    assert_eq!(
        web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&denied_id),
            Some(request.clone())
        )
        .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let mut injected = request;
    injected["allow_plan_scan"] = json!(true);
    assert!(web
        .request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&denied_id),
            Some(injected)
        )
        .status()
        .is_client_error());
    assert_eq!(
        call(&fixture, &account, &["tasks", "list"])["tasks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
