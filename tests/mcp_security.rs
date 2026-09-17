//! MCP 安全专项；仅合成配置、stdin 和本机命名管道，不读取真实账号。
#![cfg(windows)]
include!("fixtures/mcp-host/lib.rs");
#[path = "fixtures/mcp-auth/mock.rs"]
mod authenticated_mock;
#[path = "support/bootstrap.rs"]
mod runtime_cleanup;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const SECRET: &str = "SYNTHETIC_KEY_AND_SQL_DO_NOT_ECHO";

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    keys: PathBuf,
    database: PathBuf,
    pipe: String,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let database = root.path().join("source");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&database).unwrap();
        let keys = root.path().join("keys.json");
        fs::write(&keys, SECRET).unwrap();
        let config = root.path().join("PRIVATE_CONFIG_PATH.json");
        fs::write(&config, json!({"db_dir":database,"keys_file":keys,"decrypted_dir":root.path().join("decrypted"),"wechat_process":"synthetic-never-running.exe"}).to_string()).unwrap();
        let mut digest = Sha256::new();
        digest.update(b"wx-cli-runtime-v2\0");
        for path in [&config, &database, &keys, &home] {
            digest.update(
                path.canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .to_lowercase()
                    .as_bytes(),
            );
            digest.update([0]);
        }
        let pipe = format!("wx-cli-v2-{:x}", digest.finalize());
        Self {
            root,
            home,
            config,
            keys,
            database,
            pipe,
        }
    }
    fn command(&self, selected: Option<&Path>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command.env_clear().current_dir(self.root.path()).args([
            "mcp",
            "--max-frame-bytes",
            "1024",
        ]);
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "WX_CLI_HOME",
        ] {
            command.env(name, &self.home);
        }
        command.env("PATH", "");
        if let Some(path) = selected {
            command.env("WX_CLI_CONFIG", path);
        }
        command
    }
    fn untouched(&self) {
        assert_eq!(fs::read(&self.keys).unwrap(), SECRET.as_bytes());
        assert_eq!(fs::read_dir(&self.database).unwrap().count(), 0);
        assert!(!self.root.path().join("decrypted").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(runtime_cleanup::RuntimeCleanup(self.home.clone()));
    }
}

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Vec<u8>>,
    stdout: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}
impl Session {
    fn start(mut command: Command) -> Self {
        // 打印完整命令参数；环境由 fixture 固定，凭据仅位于合成文件内。
        println!("COMMAND: {command:?}");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let stdout = thread::spawn(move || {
            let mut reader = BufReader::new(out.take(1024 * 1024));
            loop {
                let mut bytes = Vec::new();
                if reader.read_until(b'\n', &mut bytes).unwrap() == 0 {
                    break;
                }
                if sender.send(bytes).is_err() {
                    break;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            err.take(64 * 1024).read_to_end(&mut bytes).unwrap();
            bytes
        });
        Self {
            child,
            input,
            replies,
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }
    fn send(&mut self, value: Value) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{value}\n").as_bytes())
            .unwrap();
    }
    fn reply(&self) -> Value {
        let bytes = self
            .replies
            .recv_timeout(Duration::from_secs(8))
            .expect("MCP 未及时响应");
        let text = String::from_utf8(bytes).unwrap();
        println!("STDOUT: {text}");
        assert!(!text.contains(SECRET) && !text.contains("PRIVATE_CONFIG_PATH"));
        serde_json::from_str(&text).expect("stdout 必须仅为 JSON-RPC")
    }
    fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"security-synthetic","version":"1"}}}));
        assert!(self.reply().get("result").is_some());
    }
    fn ready(&mut self) {
        self.initialize();
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        let result = self.reply();
        assert_eq!(result["id"], id);
        result
    }
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "MCP 未在期限内退出");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn finish(mut self, success: bool) {
        self.input.take();
        assert_eq!(self.wait().success(), success);
        self.stdout.take().unwrap().join().unwrap();
        assert!(self.replies.try_iter().next().is_none(), "存在多余协议输出");
        let stderr = String::from_utf8(self.stderr.take().unwrap().join().unwrap()).unwrap();
        println!("STDERR: {stderr}");
        assert!(!stderr.contains(SECRET) && !stderr.contains("PRIVATE_CONFIG_PATH"));
        if success {
            assert!(stderr.is_empty());
        } else {
            assert!(stderr.contains("MCP stdio transport failed"));
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.input.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(join) = self.stdout.take() {
            let _ = join.join();
        }
        if let Some(join) = self.stderr.take() {
            let _ = join.join();
        }
    }
}

struct Mock {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<JoinHandle<Vec<Value>>>,
}
impl Mock {
    fn start(pipe: &str, oversized_initial_ping: bool) -> Self {
        let pipe = pipe.to_owned();
        let (ready, wait) = mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let join = thread::spawn(move || {
            use interprocess::local_socket::{
                tokio::prelude::*, GenericNamespaced, ListenerOptions,
            };
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                let listener = ListenerOptions::new().name(pipe.clone().to_ns_name::<GenericNamespaced>().unwrap()).create_tokio().unwrap();
                ready.send(()).unwrap();
                let mut requests = Vec::new();
                let mut first_ping = true;
                let serve = async {
                    loop {
                        let mut stream = listener.accept().await.unwrap();
                        let runtime_id = pipe.strip_prefix("wx-cli-v2-").unwrap();
                        let hello = json!({"version":3,"runtime_id":runtime_id});
                        stream.write_all(format!("{hello}\n").as_bytes()).await.unwrap();
                        let mut reader = tokio::io::BufReader::new(stream.take(16 * 1024));
                        let mut line = String::new();
                        reader.read_line(&mut line).await.unwrap();
                        let envelope: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(envelope["version"], 3);
                        assert_eq!(envelope["runtime_id"], runtime_id);
                        let request = envelope["request"].clone();
                        let ping = request["cmd"] == "ping";
                        let reply = if ping && first_ping && oversized_initial_ping {
                            first_ping = false;
                            // 有限的 8 MiB 模拟响应，足以区别 1 KiB 上限与无限 read_line。
                            format!("{}\n", json!({"ok":true,"pong":true,"padding":"x".repeat(8*1024*1024)}))
                        } else if ping { "{\"ok\":true,\"pong\":true}\n".to_owned() }
                        else { "{\"ok\":true,\"contacts\":[]}\n".to_owned() };
                        requests.push(request);
                        let response: Value = serde_json::from_str(&reply).unwrap();
                        let reply = format!("{}\n", json!({"version":3,"runtime_id":runtime_id,"result":"response","response":response}));
                        // 客户端拒绝超限响应后可以断开；写入成功也可能仅代表系统缓冲。
                        let _ = reader.get_mut().get_mut().write_all(reply.as_bytes()).await;
                    }
                };
                tokio::select! {
                    _ = stopped => {},
                    _ = tokio::time::sleep(Duration::from_secs(12)) => panic!("mock exceeded deadline"),
                    _ = serve => {},
                }
                requests
            })
        });
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            stop: Some(stop),
            join: Some(join),
        }
    }
    fn finish(mut self) -> Vec<Value> {
        let _ = self.stop.take().unwrap().send(());
        self.join.take().unwrap().join().unwrap()
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[test]
fn mcp_uninitialized_and_write_tool_calls_never_pin_or_dispatch() {
    let f = Fixture::new();
    let before = fs::read(&f.config).unwrap();
    let server = Mock::start(&f.pipe, false);
    let mut wx = Session::start(f.command(Some(&f.config)));
    assert_eq!(
        wx.call(1, "get_contacts", json!({}))["error"]["code"],
        -32002
    );
    wx.initialize();
    assert_eq!(
        wx.call(2, "get_contacts", json!({}))["error"]["code"],
        -32002
    );
    wx.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    for (id, name) in ["decrypt", "export_chat", "run", "toolkit"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            wx.call(id as u64 + 3, name, json!({}))["error"]["code"],
            -32602
        );
    }
    let rejected = wx.call(9,"get_contacts",json!({"query":format!("SELECT {SECRET} FROM contact"),"output":f.root.path().join("stolen.json")}));
    assert_eq!(rejected["error"]["code"], -32602);
    // 非法工具不能提前取得配置锁，也不能触发任何后台探测。
    assert!(fs::OpenOptions::new().write(true).open(&f.config).is_ok());
    wx.finish(true);
    assert!(server.finish().is_empty());
    assert_eq!(fs::read(&f.config).unwrap(), before);
    assert!(!f.home.join("accounts").exists());
    assert!(!f.root.path().join("stolen.json").exists());
    f.untouched();
}

#[test]
fn mcp_empty_or_invalid_explicit_config_never_falls_back_or_echoes_details() {
    let f = Fixture::new();
    fs::copy(&f.config, f.home.join("config.json")).unwrap();
    fs::copy(&f.config, f.root.path().join("config.json")).unwrap();
    let malformed = f.root.path().join("malformed.json");
    let huge = f.root.path().join("huge.json");
    let implicit = f.root.path().join("implicit.json");
    fs::write(
        &malformed,
        format!("{{\"db_dir\":SELECT {SECRET} FROM contact"),
    )
    .unwrap();
    fs::File::create(&huge)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    fs::write(
        &implicit,
        json!({"keys_file":f.keys,"private":SECRET}).to_string(),
    )
    .unwrap();
    let server = Mock::start(&f.pipe, false);
    for selected in [None, Some(malformed.as_path()), Some(huge.as_path())] {
        let mut command = f.command(selected);
        if selected.is_none() {
            command.env("WX_CLI_CONFIG", "");
        }
        let mut wx = Session::start(command);
        wx.ready();
        let result = wx.call(1, "get_contacts", json!({}));
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(
            result["result"]["content"][0]["text"],
            "Query backend unavailable"
        );
        wx.finish(true);
    }
    assert!(server.finish().is_empty());
    assert!(!f.home.join("accounts").exists());
    // Match the child's APPDATA override without changing this parallel test process's environment.
    // Keep db_dir absent on disk so daemon-owned explicit-account validation still rejects it.
    let fallback_db_dir = f.home.join("Tencent/xwechat");
    let runtime = runtime::RuntimeContext::from_config(
        implicit.clone(),
        config::Config {
            key_store: None,
            db_dir: fallback_db_dir.clone(),
            keys_file: f.keys.clone(),
            decrypted_dir: f.root.path().join("decrypted"),
            wechat_process: "Weixin.exe".into(),
        },
        f.home.clone(),
    )
    .unwrap();
    assert!(!fallback_db_dir.exists());
    let directory = runtime.directory.clone();
    let server = authenticated_mock::Mock::start(runtime, |_| {
        panic!("implicit account must be rejected before any business query")
    });
    let mut wx = Session::start(f.command(Some(&implicit)));
    wx.ready();
    let result = wx.call(1, "get_contacts", json!({}));
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["content"][0]["text"],
        "Query backend unavailable"
    );
    wx.finish(true);
    assert_eq!(
        server.mcp_calls(),
        1,
        "must reach authenticated MCP dispatch"
    );
    assert_eq!(server.finish(), vec![json!({"cmd":"ping"})]);
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    files.sort();
    assert!(
        files.is_empty(),
        "mock identity files must be removed at shutdown"
    );
    assert!(!fallback_db_dir.exists());
    f.untouched();
}

#[test]
fn mcp_fixture_cleanup_stops_daemon_after_configuration_changes() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    // Only this lifecycle fixture needs a real query daemon. Keep the legacy
    // sentinel untouched and seed an isolated production DPAPI store directly.
    let mut document: Value = serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    document["key_store"] = json!(f.root.path().join("synthetic-keys.dpapi"));
    fs::write(&f.config, serde_json::to_vec(&document).unwrap()).unwrap();
    let runtime = fixture_runtime(&f.config, &f.home).unwrap();
    key_store::seed_databases(&runtime, json!({}));
    let mut wx = Session::start(f.command(Some(&f.config)));
    wx.ready();
    let reply = wx.call(1, "get_contacts", json!({}));
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["content"][0]["text"], "Query failed");
    wx.finish(true);
    assert!(
        runtime.pid_path().is_file(),
        "must exercise a real daemon, not a rejected launch"
    );
    fs::write(&f.config, b"invalid changed configuration").unwrap();
    drop(runtime_cleanup::RuntimeCleanup(f.home.clone()));
    assert!(!runtime.pid_path().exists());
    assert!(fs::OpenOptions::new()
        .write(true)
        .share_mode(0)
        .open(runtime.directory.join("daemon.lock"))
        .is_ok());
    f.untouched();
}

#[test]
fn mcp_mock_cleanup_allows_restarting_the_same_runtime() {
    let f = Fixture::new();
    let runtime = fixture_runtime(&f.config, &f.home).unwrap();
    for _ in 0..2 {
        let server = authenticated_mock::Mock::start(runtime.clone(), |_| {
            panic!("no business calls expected")
        });
        assert!(server.finish().is_empty());
        assert!(!runtime.pid_path().exists());
        assert!(!runtime.directory.join("service-token.key").exists());
    }
}

#[test]
fn mcp_oversized_unterminated_stdin_exits_before_eof() {
    let f = Fixture::new();
    let mut wx = Session::start(f.command(None));
    let input = format!("{SECRET}{}", "x".repeat(1025));
    wx.input
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    // 有意保持 stdin 打开；只有帧限额而非 EOF 才能触发退出。
    assert!(!wx.wait().success());
    wx.finish(false);
    assert!(!f.home.join("accounts").exists());
    f.untouched();
}

#[test]
fn mcp_startup_ping_must_obey_response_byte_limit() {
    let f = Fixture::new();
    let runtime = fixture_runtime(&f.config, &f.home).unwrap();
    let directory = runtime.directory.clone();
    assert_eq!(runtime.pipe_name(), f.pipe);
    let oversized = format!(
        "{}\n",
        json!({"ok":true,"pong":true,"padding":"x".repeat(8*1024*1024)})
    )
    .into_bytes();
    let server =
        authenticated_mock::Mock::start_with_initial_ping(runtime, Some(oversized), |request| {
            assert_eq!(request, &json!({"cmd":"contacts","limit":50}));
            authenticated_mock::Reply::Json(json!({"ok":true,"contacts":[]}))
        });
    let mut wx = Session::start(f.command(Some(&f.config)));
    wx.ready();
    let reply = wx.call(1, "get_contacts", json!({}));
    wx.finish(true);
    assert_eq!(
        server.mcp_calls(),
        1,
        "must reach authenticated MCP dispatch"
    );
    let requests = server.finish();
    assert!(!directory.join("daemon.pid").exists());
    assert!(!directory.join("service-token.key").exists());
    println!("IPC REQUEST SEQUENCE: {requests:?}");
    println!("STARTUP LOCK CREATED: {}", f.home.join("accounts").exists());
    f.untouched();
    // 初始超大 Pong 若被当作有效健康响应，将直接进入 contacts，缺少第二次 ping。
    // 重试的小 Pong 必须成功，避免把任意连接失败误认为完整的安全回归通过。
    assert_eq!(
        requests,
        vec![
            json!({"cmd":"ping"}),
            json!({"cmd":"ping"}),
            json!({"cmd":"contacts","limit":50})
        ],
        "必须拒绝初始 8 MiB Pong，再通过小 Pong 健康探测后查询"
    );
    assert_eq!(reply["result"]["isError"], false);
    let content: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(content, json!({"contacts":[]}));
}
