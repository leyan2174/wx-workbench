use super::{success, Fixture};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn call(fixture: &Fixture, account: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&success(fixture.run(account, args))).unwrap()
}

fn snapshot(fixture: &Fixture) -> PathBuf {
    let destination = fixture.root.join("enterprise-snapshot");
    fs::create_dir(&destination).unwrap();
    for name in ["message.db", "user.db", "session.db"] {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/enterprise/queries")
                .join(name),
            destination.join(name),
        )
        .unwrap();
    }
    destination
}

pub(super) fn assert_personal_tasks(fixture: &Fixture, account: &Path, user: &str) {
    let task = call(fixture, account, &["tasks", "submit", "wechat-decrypt", "--wait"]);
    assert_eq!(task["status"], "succeeded");
    assert!(account.join("decrypted/contact/contact.db").is_file());
    let task = call(fixture, account, &["tasks", "submit", "export-all", "--users", user, "--formats", "json", "--no-images", "--wait"]);
    assert_eq!(task["status"], "succeeded");
    let mut directories = vec![PathBuf::from(task["output_dir"].as_str().unwrap()).join("chats")];
    let mut found = false;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() { directories.push(entry.path()); }
            else if entry.path().extension().is_some_and(|extension| extension == "json") {
                found |= fs::read_to_string(entry.path()).unwrap().contains("synthetic message");
            }
        }
    }
    assert!(found, "personal daemon export omitted the synthetic message");
}

fn terminal(fixture: &Fixture, account: &Path, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let task = call(fixture, account, &["tasks", "get", id]);
        if matches!(
            task["status"].as_str(),
            Some("succeeded" | "failed" | "cancelled" | "interrupted")
        ) {
            return task;
        }
        assert!(Instant::now() < deadline, "task did not finish: {task}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn daemon_task_worker_exports_once_and_history_survives_restart() {
    let mut fixture = Fixture::new();
    let account = fixture.account("task-owner", false);
    fs::remove_file(account.join("all_keys.json")).unwrap();
    let other = fixture.account("other-task-owner", false);
    let snapshot = snapshot(&fixture);
    let before = fs::read(snapshot.join("message.db")).unwrap();
    let info = call(
        &fixture,
        &account,
        &[
            "tasks",
            "configure",
            "--enterprise-snapshot",
            snapshot.to_str().unwrap(),
        ],
    );
    assert_eq!(info["configured"], true);
    assert!(!fixture
        .run(&account, &["tasks", "configure"])
        .status
        .success());
    let id = "ab".repeat(32);
    let args = [
        "tasks",
        "submit",
        "wxwork-export",
        "--all-conversations",
        "--formats",
        "json,csv,html",
        "--request-id",
        &id,
        "--wait",
    ];
    let task = call(&fixture, &account, &args);
    assert_eq!(task["status"], "succeeded", "{task}");
    let output = Path::new(task["output_dir"].as_str().unwrap()).join("enterprise-export");
    assert!(output.is_dir());
    assert!(fs::read_dir(&output).unwrap().next().is_some());
    assert_eq!(call(&fixture, &account, &args), task);
    assert_eq!(
        call(&fixture, &account, &["tasks", "list"])["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!fixture
        .run(
            &account,
            &["tasks", "submit", "wechat-decrypt", "--request-id", &id]
        )
        .status
        .success());
    assert!(!fixture.run(&other, &["tasks", "get", &id]).status.success());
    let logs = call(&fixture, &account, &["tasks", "logs", &id]);
    assert!(!logs["logs"].as_array().unwrap().is_empty());
    assert_eq!(fs::read(snapshot.join("message.db")).unwrap(), before);
    success(fixture.run(&account, &["daemon", "stop"]));
    assert_eq!(call(&fixture, &account, &["tasks", "get", &id]), task);
    call(
        &fixture,
        &account,
        &[
            "tasks",
            "configure",
            "--enterprise-snapshot",
            snapshot.to_str().unwrap(),
        ],
    );
    assert_eq!(call(&fixture, &account, &args), task);
}

struct Web {
    child: Child,
    origin: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Web {
    fn start(fixture: &Fixture, account: &Path, snapshot: &Path) -> Self {
        use std::os::windows::process::CommandExt;
        let log = fixture.root.join("web-process.log");
        let stdout = fs::File::create(&log).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_wx"))
            .args([
                "toolkit",
                "web",
                "--port",
                "0",
                "--enterprise-snapshot",
                snapshot.to_str().unwrap(),
            ])
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_DAEMON_TASK_WORKER")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env("WX_CLI_CONFIG", account.join("config.json"))
            .env("WX_CLI_HOME", fixture.root.join("shared-runtime"))
            .current_dir(&fixture.root)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stderr(stdout.try_clone().unwrap())
            .stdout(stdout)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let (origin, token) = loop {
            let text = fs::read_to_string(&log).unwrap();
            if let Some(start) = text.find("http://127.0.0.1:") {
                let url = text[start..].lines().next().unwrap().trim();
                if let Some((origin, token)) = url.split_once("/#token=") {
                    break (origin.to_owned(), token.to_owned());
                }
            }
            if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Web startup failed: {text}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        Self {
            child,
            origin,
            token,
            http: reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        id: Option<&str>,
        body: Option<Value>,
    ) -> reqwest::blocking::Response {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.origin))
            .header("x-wx-token", &self.token)
            .header("origin", &self.origin);
        if let Some(id) = id {
            request = request.header("idempotency-key", id);
        }
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        request.send().unwrap()
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn web_and_cli_share_tasks_and_web_shutdown_does_not_stop_daemon() {
    let mut fixture = Fixture::new();
    let account = fixture.account("web-task-owner", false);
    let snapshot = snapshot(&fixture);
    let mut web = Web::start(&fixture, &account, &snapshot);
    let info = call(&fixture, &account, &["tasks", "info"]);
    let directory = fixture
        .root
        .join("shared-runtime/accounts")
        .join(info["runtime_id"].as_str().unwrap());
    let pid = fs::read(directory.join("daemon.pid")).unwrap();
    assert_eq!(
        web.http
            .get(format!("{}/api/tasks", web.origin))
            .send()
            .unwrap()
            .status(),
        401
    );
    let id = "cd".repeat(32);
    let request =
        json!({"kind":"wxwork_export","options":{"all_conversations":true,"formats":["json"]}});
    for _ in 0..2 {
        let response = web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&id),
            Some(request.clone()),
        );
        let status = response.status();
        let text = response.text().unwrap();
        assert_eq!(status, 202, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["id"], id);
    }
    assert_eq!(
        call(&fixture, &account, &["tasks", "list"])["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(web
        .request(
            reqwest::Method::POST,
            "/api/shutdown",
            None,
            Some(json!({}))
        )
        .status()
        .is_success());
    let deadline = Instant::now() + Duration::from_secs(10);
    while web.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "Web did not close");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read(directory.join("daemon.pid")).unwrap(), pid);
    let task = terminal(&fixture, &account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(call(&fixture, &account, &["tasks", "cancel", &id]), task);
    let web = Web::start(&fixture, &account, &snapshot);
    let result = web.request(
        reqwest::Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        None,
    );
    assert_eq!(result.status(), 200);
    assert_eq!(
        serde_json::from_str::<Value>(&result.text().unwrap()).unwrap(),
        task
    );
    success(fixture.run(&account, &["daemon", "stop"]));
    // Cross several monitor ticks: an open Web client must not undo explicit stop.
    std::thread::sleep(Duration::from_secs(5));
    assert!(!directory.join("daemon.pid").exists());
    assert!(!directory.join("service-token.key").exists());
    assert!(web
        .request(reqwest::Method::GET, "/api/tasks", None, None)
        .status()
        .is_server_error());
}

fn worker_pid(parent: u32) -> Option<u32> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32ParentProcessID == parent {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[test]
fn cancelling_and_stopping_reap_running_workers_without_stopping_queries_early() {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    let mut fixture = Fixture::new();
    let account = fixture.account("cancel-task-owner", true);
    let snapshot = snapshot(&fixture);
    let mut db = rusqlite::Connection::open(snapshot.join("message.db")).unwrap();
    let transaction = db.transaction().unwrap();
    {
        let mut insert = transaction.prepare("INSERT INTO message_table VALUES(?1,?1,?1,10000000001,'R:team',2,1700000000,0,?2,NULL,NULL)").unwrap();
        for id in 100..100100 {
            insert
                .execute(rusqlite::params![
                    id,
                    "synthetic cancellation workload".repeat(4)
                ])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    drop(db);
    let info = call(
        &fixture,
        &account,
        &[
            "tasks",
            "configure",
            "--enterprise-snapshot",
            snapshot.to_str().unwrap(),
        ],
    );
    let directory = fixture
        .root
        .join("shared-runtime/accounts")
        .join(info["runtime_id"].as_str().unwrap());
    let pid_record: Value =
        serde_json::from_slice(&fs::read(directory.join("daemon.pid")).unwrap()).unwrap();
    let daemon_pid = pid_record["pid"].as_u64().unwrap() as u32;
    for stop in [false, true] {
        let id = if stop { "ef" } else { "de" }.repeat(32);
        call(
            &fixture,
            &account,
            &[
                "tasks",
                "submit",
                "wxwork-export",
                "--all-conversations",
                "--formats",
                "json,csv,html",
                "--request-id",
                &id,
            ],
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Some(pid) = worker_pid(daemon_pid) {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "worker never started: {}",
                call(&fixture, &account, &["tasks", "get", &id])
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid).unwrap() };
        let _owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        assert!(
            success(fixture.run(&account, &["contacts", "--json"])).contains("cancel-task-owner")
        );
        if stop {
            success(fixture.run(&account, &["daemon", "stop"]));
            assert!(!directory.join("daemon.pid").exists());
            assert!(!directory.join("service-token.key").exists());
        } else {
            call(&fixture, &account, &["tasks", "cancel", &id]);
        }
        assert_eq!(
            unsafe { WaitForSingleObject(HANDLE(handle.0), 2000) },
            WAIT_OBJECT_0,
            "worker survived cancellation"
        );
        let task = terminal(&fixture, &account, &id);
        assert_eq!(task["status"], "cancelled", "{task}");
        if !stop {
            let current: Value =
                serde_json::from_slice(&fs::read(directory.join("daemon.pid")).unwrap()).unwrap();
            assert_eq!(current, pid_record);
        }
    }
}
