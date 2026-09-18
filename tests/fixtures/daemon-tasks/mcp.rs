//! Real stdio MCP -> authenticated service -> daemon queue -> private worker.
use super::{call, terminal, Web};
use crate::{success, Fixture};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<Value>,
    next: u64,
}
impl Mcp {
    fn start(fixture: &Fixture, account: &Path, flags: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wx"))
            .arg("mcp")
            .args(flags)
            .env_clear()
            .envs(
                ["SystemRoot", "WINDIR", "TEMP", "TMP", "USERPROFILE"]
                    .into_iter()
                    .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
            )
            .env("WX_CLI_CONFIG", account.join("config.json"))
            .env("WX_CLI_HOME", fixture.root.join("shared-runtime"))
            .current_dir(&fixture.root)
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (send, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        let mut mcp = Self {
            input: child.stdin.take(),
            child,
            replies,
            next: 0,
        };
        mcp.request("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic-tasks","version":"1"}}));
        mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        mcp
    }
    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params}));
        let reply = self
            .replies
            .recv_timeout(Duration::from_secs(40))
            .expect("MCP reply deadline");
        assert_eq!(reply["id"], self.next, "{reply}");
        reply
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
    }
    fn data(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.tool(name, arguments);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        reply["result"]["structuredContent"].clone()
    }
    fn close(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "MCP did not close");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const DECRYPT: &[&str] = &["--tasks", "--task-kind", "wechat_decrypt"];

#[test]
fn mcp_tasks_real_worker_shared_with_cli_web_and_retry_after_disconnect() {
    let mut fixture = Fixture::new();
    let account = fixture.account("mcp-task-owner", true);
    let mut mcp = Mcp::start(&fixture, &account, DECRYPT);
    let tools = mcp.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 28);
    let id = "a1".repeat(32);
    let args = json!({"idempotency_key":id,"kind":"wechat_decrypt"});
    let task = mcp.data("submit_task", args.clone());
    assert_eq!(task["id"], id);
    assert!(matches!(
        task["status"].as_str(),
        Some("queued" | "running")
    ));
    // Drop the stdio session, not the daemon task. Subsequent CLI reads prove ownership.
    mcp.close();
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    let db = rusqlite::Connection::open(account.join("decrypted/contact/contact.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT username FROM contact", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "mcp-task-owner"
    );
    drop(db);
    let mut mcp = Mcp::start(&fixture, &account, DECRYPT);
    assert_eq!(mcp.data("submit_task", args.clone()), task);
    assert_eq!(mcp.data("get_task", json!({"id":id})), task);
    assert_eq!(
        mcp.data("list_tasks", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(mcp.data("cancel_task", json!({"id":id})), task);
    let events = mcp.data(
        "get_task_events",
        json!({"after":0,"limit":128,"wait_ms":0}),
    );
    assert_eq!(events["reset"], false, "{events}");
    assert!(events["cursor"].as_u64().unwrap() > 0);
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event.to_string().contains(&id)));
    assert_eq!(
        mcp.data(
            "get_task_events",
            json!({"after":u64::MAX,"limit":1,"wait_ms":0})
        )["reset"],
        true
    );
    let query = mcp.tool("get_contacts", json!({}));
    assert_ne!(query["result"]["isError"], true, "{query}");
    assert!(query.to_string().contains("mcp-task-owner"));
    let web = Web::start(&fixture, &account);
    assert_eq!(
        serde_json::from_str::<Value>(
            &web.request(
                reqwest::Method::GET,
                &format!("/api/tasks/{id}"),
                None,
                None
            )
            .text()
            .unwrap()
        )
        .unwrap(),
        task
    );
    let cli_id = "a2".repeat(32);
    assert_eq!(
        serde_json::from_str::<Value>(
            &web.request(
                reqwest::Method::POST,
                &format!("/api/tasks/{id}/cancel"),
                None,
                Some(json!({}))
            )
            .text()
            .unwrap()
        )
        .unwrap(),
        task
    );
    call(
        &fixture,
        &account,
        &["tasks", "submit", "wechat_decrypt", "--request-id", &cli_id],
    );
    assert_eq!(mcp.data("get_task", json!({"id":cli_id}))["id"], cli_id);
    mcp.data("cancel_task", json!({"id":cli_id}));
    let web_id = "a3".repeat(32);
    assert_eq!(
        web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&web_id),
            Some(json!({"kind":"wechat_decrypt"}))
        )
        .status(),
        202
    );
    assert_eq!(mcp.data("get_task", json!({"id":web_id}))["id"], web_id);
    call(&fixture, &account, &["tasks", "cancel", &web_id]);

    // Discard a submitted response entirely. Observe daemon acceptance through CLI,
    // then terminate that MCP host and retry with the same id on a fresh connection.
    let lost_id = "a4".repeat(32);
    let lost = json!({"idempotency_key":lost_id,"kind":"wechat_decrypt"});
    mcp.send(json!({"jsonrpc":"2.0","id":900,"method":"tools/call","params":{"name":"submit_task","arguments":lost}}));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if fixture
            .run(&account, &["tasks", "get", &lost_id])
            .status
            .success()
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    drop(mcp);
    let mut mcp = Mcp::start(&fixture, &account, DECRYPT);
    assert_eq!(mcp.data("submit_task", lost)["id"], lost_id);
    assert_eq!(
        mcp.data("list_tasks", json!({}))["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["id"] == lost_id)
            .count(),
        1
    );
    mcp.close();
    drop(web);
    terminal(&fixture, &account, &lost_id);
    success(fixture.run(&account, &["daemon", "stop"]));
    let mut restarted = Mcp::start(&fixture, &account, DECRYPT);
    assert_eq!(restarted.data("submit_task", args), task);
}

#[test]
fn mcp_tasks_reject_model_authorization_paths_and_changed_account() {
    let mut fixture = Fixture::new();
    let account = fixture.account("mcp-task-policy", true);
    let store = account.join("keys.dpapi");
    let store_before = fs::read(&store).unwrap();
    assert!(store_before.starts_with(b"WXKEYS\0\x01"));
    let mut mcp = Mcp::start(&fixture, &account, DECRYPT);
    for bad in [
        json!({"idempotency_key":"a".repeat(64),"kind":"shell"}),
        json!({"idempotency_key":"a".repeat(64),"kind":"wechat_decrypt","output_dir":"C:/outside"}),
        json!({"idempotency_key":"bad","kind":"wechat_decrypt"}),
        json!({"idempotency_key":"a".repeat(64),"kind":"export_all","options":{"with_transcriptions":true}}),
        json!({"idempotency_key":"a".repeat(64),"kind":"export_all","options":{"allow_upload":true}}),
        json!({"idempotency_key":"a".repeat(64),"kind":"export_all","options":{"with_transcriptions":true,"allow_upload":true}}),
    ] {
        assert_eq!(mcp.tool("submit_task", bad)["error"]["code"], -32602);
    }
    for (kind, options) in [
        ("wechat_keys", json!({"authorize_memory_scan":true})),
        ("export_all", json!({})),
        ("decode_images", json!({})),
    ] {
        let reply = mcp.tool(
            "submit_task",
            json!({"idempotency_key":"a".repeat(64),"kind":kind,"options":options}),
        );
        assert_eq!(
            reply["result"]["structuredContent"]["error"]["code"], "host_forbidden",
            "{reply}"
        );
    }
    match fs::read_dir(fixture.root.join("shared-runtime/accounts")) {
        Ok(entries) => {
            for entry in entries {
                assert!(
                    !entry.unwrap().path().join("daemon.pid").exists(),
                    "Rejected host requests must not start a daemon"
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("Cannot inspect daemon state: {error}"),
    }
    assert_eq!(fs::read(&store).unwrap(), store_before);
    mcp.data("list_tasks", json!({}));
    let original = fs::read(account.join("config.json")).unwrap();
    let mut config: Value = serde_json::from_slice(&original).unwrap();
    config["decrypted_dir"] = json!("different-output");
    fs::write(account.join("config.json"), config.to_string()).unwrap();
    let rejected = mcp.tool("list_tasks", json!({}));
    assert_eq!(
        rejected["result"]["structuredContent"]["error"]["code"], "configuration_changed",
        "{rejected}"
    );
    fs::write(account.join("config.json"), original).unwrap();
    assert_eq!(
        mcp.tool("list_tasks", json!({}))["result"]["structuredContent"]["error"]["code"],
        "configuration_changed"
    );
    mcp.close();
    let other = fixture.account("mcp-other-account", true);
    let mut mcp = Mcp::start(&fixture, &account, DECRYPT);
    mcp.data("list_tasks", json!({}));
    let original = fs::read(account.join("config.json")).unwrap();
    let mut config: Value = serde_json::from_slice(&original).unwrap();
    config["db_dir"] = json!(other.join("db_storage"));
    fs::write(account.join("config.json"), config.to_string()).unwrap();
    assert_eq!(
        mcp.tool("list_tasks", json!({}))["result"]["structuredContent"]["error"]["code"],
        "configuration_changed"
    );
    fs::write(account.join("config.json"), original).unwrap();
    mcp.close();

    // Settings cannot be silently changed by a different entry point.
    let cache = fixture.root.join("another-cache");
    fs::create_dir(&cache).unwrap();
    let mut mcp = Mcp::start(
        &fixture,
        &account,
        &["--tasks", "--task-image-cache-dir", cache.to_str().unwrap()],
    );
    let reply = mcp.tool("list_tasks", json!({}));
    assert_eq!(
        reply["result"]["structuredContent"]["error"]["code"], "settings_conflict",
        "{reply}"
    );
}

#[test]
fn mcp_tasks_cancel_reaps_worker_and_crash_restores_interrupted() {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{
                GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
                WaitForSingleObject, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            },
        },
    };
    let mut fixture = Fixture::new();
    let account = fixture.account("mcp-lifecycle", true);
    super::personal_messages(&fixture, &account, 100_000);
    let mut mcp = Mcp::start(
        &fixture,
        &account,
        &[
            "--tasks",
            "--task-kind",
            "export_all",
            "--task-allow-media-write",
        ],
    );
    mcp.data("list_tasks", json!({}));
    let info = call(&fixture, &account, &["tasks", "info"]);
    let directory = fixture
        .root
        .join("shared-runtime/accounts")
        .join(info["runtime_id"].as_str().unwrap());
    let record: Value =
        serde_json::from_slice(&fs::read(directory.join("daemon.pid")).unwrap()).unwrap();
    let pid = record["pid"].as_u64().unwrap() as u32;
    let daemon = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            false,
            pid,
        )
        .unwrap()
    };
    let _daemon_owned = unsafe { OwnedHandle::from_raw_handle(daemon.0) };
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(daemon, &mut created, &mut exited, &mut kernel, &mut user).unwrap();
    }
    assert_eq!(
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        record["created"].as_u64().unwrap()
    );
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            daemon,
            PROCESS_NAME_FORMAT(0),
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )
        .unwrap();
    }
    assert_eq!(
        fs::canonicalize(String::from_utf16(&path[..len as usize]).unwrap()).unwrap(),
        fs::canonicalize(env!("CARGO_BIN_EXE_wx")).unwrap()
    );

    for crash in [false, true] {
        let id = if crash { "c2" } else { "c1" }.repeat(32);
        let args = json!({"idempotency_key":id,"kind":"export_all","options":{"users":["task-peer"],"include_images":false,"formats":["json","csv","html"]}});
        mcp.data("submit_task", args.clone());
        let deadline = Instant::now() + Duration::from_secs(8);
        let worker = loop {
            if let Some(pid) = super::worker_pid(pid) {
                if let Ok(handle) = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
                    break handle;
                }
            }
            assert!(Instant::now() < deadline, "No daemon-owned task worker");
            std::thread::sleep(Duration::from_millis(5));
        };
        let _worker_owned = unsafe { OwnedHandle::from_raw_handle(worker.0) };
        if crash {
            assert_eq!(mcp.data("get_task", json!({"id":id}))["status"], "running");
            unsafe {
                TerminateProcess(daemon, 73).unwrap();
            }
            assert_eq!(unsafe { WaitForSingleObject(daemon, 5000) }, WAIT_OBJECT_0);
        } else {
            mcp.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":mcp.next,"reason":"host no longer waiting"}}));
            mcp.close();
            assert_eq!(
                unsafe { WaitForSingleObject(worker, 0) },
                WAIT_TIMEOUT,
                "MCP disconnect cancelled the worker"
            );
            mcp = Mcp::start(
                &fixture,
                &account,
                &[
                    "--tasks",
                    "--task-kind",
                    "export_all",
                    "--task-allow-media-write",
                ],
            );
            mcp.data("cancel_task", json!({"id":id}));
        }
        assert_eq!(
            unsafe { WaitForSingleObject(worker, 5000) },
            WAIT_OBJECT_0,
            "Worker was not reaped"
        );
        let task = terminal(&fixture, &account, &id);
        assert_eq!(
            task["status"],
            if crash { "interrupted" } else { "cancelled" },
            "{task}"
        );
        if crash {
            assert_eq!(
                mcp.data("submit_task", args),
                task,
                "Retry must not restart interrupted work"
            );
        }
    }
}
