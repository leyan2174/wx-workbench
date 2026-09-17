//! 真实 wx mcp 黑盒回归：只使用临时合成账号和命名管道 mock，不读取真实微信数据。
//! 认证 service mock 调用真实 daemon MCP 业务，仅查询结果可注入；30秒超时用实钟验收。
#![cfg(windows)]

include!("fixtures/mcp-voice-host/lib.rs");
#[path = "fixtures/mcp-auth/mock.rs"]
mod authenticated_mock;
#[path = "support/bootstrap.rs"]
mod runtime_cleanup;
use authenticated_mock::{Mock, Reply};

#[allow(dead_code)]
#[path = "fixtures/mcp-voice-runtime/artifacts.rs"]
mod voice;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TOOLS: [&str; 17] = [
    "get_recent_sessions",
    "get_contacts",
    "get_chat_history",
    "search_messages",
    "decode_transfer",
    "decode_location",
    "get_new_messages",
    "get_chat_images",
    "decode_refer",
    "get_contact_tags",
    "get_tag_members",
    "get_voice_messages",
    "decode_file_message",
    "decode_record_item",
    "decode_image",
    "decode_voice",
    "transcribe_voice",
];
const REQUIRED_VOICE_ARGS: [&str; 2] = ["decode_voice", "transcribe_voice"];

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
}

struct Account {
    config: PathBuf,
    keys: PathBuf,
    database: PathBuf,
    pipe: String,
    runtime: runtime::RuntimeContext,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(runtime_cleanup::RuntimeCleanup(self.home.clone()));
    }
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("runtime");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(temp.path().join("empty-path")).unwrap();
        Self { temp, home }
    }

    fn account(&self, name: &str) -> Account {
        let profile = self.temp.path().join(name);
        let database = profile.join("db_storage");
        fs::create_dir_all(&database).unwrap();
        let keys = profile.join("synthetic-keys.json");
        // 故意不是密钥JSON；若错误走到解密流程也不能得到可用数据库。
        fs::write(&keys, b"synthetic must-not-load key sentinel").unwrap();
        let config = profile.join("config.json");
        fs::write(
            &config,
            json!({
                "db_dir":database,"keys_file":keys,"decrypted_dir":profile.join("decrypted"),
                "wechat_process":"synthetic-not-running.exe"
            })
            .to_string(),
        )
        .unwrap();
        // 独立的 v2 运行身份契约 oracle，所有参与路径都已存在，无需模仿缺失祖先解析。
        let mut digest = Sha256::new();
        digest.update(b"wx-cli-runtime-v2\0");
        for path in [&config, &database, &keys, &self.home] {
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
        let runtime = fixture_runtime(&config, &self.home).unwrap();
        assert_eq!(runtime.pipe_name(), pipe);
        Account {
            runtime,
            config,
            keys,
            database,
            pipe,
        }
    }

    fn command(&self, account: Option<&Account>, limit: Option<usize>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command.env_clear().current_dir(self.temp.path()).arg("mcp");
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            command.env(key, &self.home);
        }
        command
            .env("PATH", self.temp.path().join("empty-path"))
            .env("WX_CLI_HOME", &self.home)
            // MCP新接口应使用context期限，而非继承普通send的环境期限。
            .env("WX_CLI_REQUEST_TIMEOUT_SECS", "1");
        if let Some(account) = account {
            command.env("WX_CLI_CONFIG", &account.config);
        }
        if let Some(limit) = limit {
            command.args(["--max-frame-bytes", &limit.to_string()]);
        }
        command
    }

    fn assert_no_daemon_or_database_output(&self, account: &Account) {
        if account.runtime.directory.exists() {
            for entry in fs::read_dir(&account.runtime.directory).unwrap() {
                let name = entry.unwrap().file_name();
                assert!(
                    matches!(name.to_str(), Some("daemon.pid" | "service-token.key")),
                    "unexpected daemon artifact: {name:?}"
                );
            }
        }
        assert_eq!(fs::read_dir(&account.database).unwrap().count(), 0);
        assert_eq!(
            fs::read(&account.keys).unwrap(),
            b"synthetic must-not-load key sentinel"
        );
        assert!(!account.config.parent().unwrap().join("decrypted").exists());
    }
}

struct Wx {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<Vec<u8>>,
    stdout: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

struct Finished {
    status: ExitStatus,
    stdout_tail: Vec<Vec<u8>>,
    stderr: String,
}

impl Wx {
    fn start(mut command: Command) -> Self {
        eprintln!("执行命令：{command:?}");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let out = child.stdout.take().unwrap();
        let mut err = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        let stdout = thread::spawn(move || {
            let mut reader = BufReader::new(out);
            loop {
                let mut line = Vec::new();
                if reader.read_until(b'\n', &mut line).unwrap() == 0 {
                    break;
                }
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            err.read_to_end(&mut bytes).unwrap();
            bytes
        });
        Self {
            child,
            stdin,
            lines,
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.stdin.as_mut().unwrap().write_all(bytes).unwrap();
    }
    fn send(&mut self, value: Value) {
        eprintln!("MCP 标准输入：{value}");
        self.raw(format!("{value}\n").as_bytes());
    }

    fn reply(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(40))
            .expect("wx failed to produce a bounded-time reply");
        eprintln!("MCP 标准输出：{}", String::from_utf8_lossy(&line));
        assert!(line.ends_with(b"\n"), "partial JSON-RPC output");
        let reply: Value =
            serde_json::from_slice(&line).expect("stdout must contain JSON-RPC only");
        assert_eq!(reply["jsonrpc"], "2.0");
        reply
    }

    fn initialize(&mut self) {
        self.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}));
        let reply = self.reply();
        assert_eq!(reply["id"], "init");
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            reply["result"]["capabilities"],
            json!({"tools":{"listChanged":false}})
        );
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }

    fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        let reply = self.reply();
        assert_eq!(reply["id"], id);
        reply
    }

    fn finish(mut self) -> Finished {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "wx failed to exit after EOF");
            thread::sleep(Duration::from_millis(10));
        };
        self.stdout.take().unwrap().join().unwrap();
        let stderr = String::from_utf8(self.stderr.take().unwrap().join().unwrap()).unwrap();
        let stdout_tail: Vec<_> = self.lines.try_iter().collect();
        for line in &stdout_tail {
            eprintln!("MCP 剩余标准输出：{}", String::from_utf8_lossy(line));
        }
        eprintln!("退出码：{status}\n完整错误输出：\n{stderr}");
        Finished {
            status,
            stdout_tail,
            stderr,
        }
    }
}

impl Drop for Wx {
    fn drop(&mut self) {
        self.stdin.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(handle) = self.stdout.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr.take() {
            let _ = handle.join();
        }
    }
}

fn success(finished: Finished) {
    assert!(finished.status.success(), "{}", finished.stderr);
    assert!(
        finished.stdout_tail.is_empty(),
        "unexpected extra protocol output"
    );
    assert!(
        finished.stderr.is_empty(),
        "unexpected stderr: {}",
        finished.stderr
    );
}

fn tool_error(reply: &Value, expected: &str) {
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["content"][0]["text"], expected);
    assert!(!reply.to_string().contains("SYNTHETIC_SECRET"));
}

#[test]
fn real_wx_initializes_lists_seventeen_tools_without_account_and_never_falls_back() {
    let fixture = Fixture::new();
    let account = fixture.account("default-decoy");
    // 即便默认发现位置有配置，没有显式WX_CLI_CONFIG也不得尝试发送。
    fs::copy(&account.config, fixture.temp.path().join("config.json")).unwrap();
    fs::copy(&account.config, fixture.home.join("config.json")).unwrap();
    let mut wx = Wx::start(fixture.command(None, None));
    wx.initialize();
    wx.send(json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}));
    let list = wx.reply();
    let names: Vec<_> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // 工具清单顺序是客户端兼容契约；语音工具位于清单末尾。
    assert_eq!(names.len(), 17);
    assert_eq!(&names[..8], &TOOLS[..8]);
    assert_eq!(
        &names[8..],
        &[
            "get_contact_tags",
            "get_tag_members",
            "decode_refer",
            "get_voice_messages",
            "decode_file_message",
            "decode_record_item",
            "decode_image",
            "decode_voice",
            "transcribe_voice",
        ]
    );
    assert_eq!(names.last(), Some(&"transcribe_voice"));
    let image = &list["result"]["tools"][14];
    assert_eq!(image["annotations"]["readOnlyHint"], false);
    assert_eq!(image["annotations"]["destructiveHint"], false);
    assert_eq!(image["inputSchema"]["additionalProperties"], false);
    assert!(image["inputSchema"]["properties"]
        .get("output_root")
        .is_none());
    for tool in &list["result"]["tools"].as_array().unwrap()[15..] {
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 2);
        assert!(properties.contains_key("chat_name"));
        assert_eq!(properties["local_id"]["minimum"], 1);
        assert_eq!(
            tool["inputSchema"]["required"],
            json!(["chat_name", "local_id"])
        );
    }
    let original: Vec<_> = names
        .iter()
        .copied()
        .filter(|name| TOOLS[..8].contains(name))
        .collect();
    assert_eq!(original, TOOLS[..8]);
    let mut actual = names;
    let mut expected = TOOLS;
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    for (index, name) in REQUIRED_VOICE_ARGS.iter().enumerate() {
        let reply = wx.call(index as i64, name, json!({}));
        assert_eq!(reply["error"]["code"], -32602);
    }
    tool_error(
        &wx.call(100, "get_contacts", json!({})),
        "Query backend unavailable",
    );
    success(wx.finish());
    fixture.assert_no_daemon_or_database_output(&account);
}

#[test]
fn real_wx_routes_all_seventeen_tools_to_selected_pipe_and_locks_account_changes() {
    let fixture = Fixture::new();
    let a = fixture.account("a");
    let b = fixture.account("b");
    assert_ne!(a.pipe, b.pipe);
    let server_a = Mock::start(a.runtime.clone(), |request| {
        Reply::Json(match request["cmd"].as_str().unwrap() {
            "sessions" => json!({"ok":true,"sessions":[]}),
            "contacts" => json!({"ok":true,"contacts":[{"username":"synthetic-a"}]}),
            "history" => json!({"ok":true,"messages":[],"count":0}),
            "search" => json!({"ok":true,"results":[],"count":0}),
            "decode_transfer" | "decode_location" => {
                json!({"ok":true,"exit_code":0,"text":"synthetic metadata"})
            }
            "attachments" => json!({"ok":true,"attachments":[],"count":0}),
            "decode_file_message" => {
                json!({"ok":true,"exit_code":0,"filename":"合成文件.txt","size":12})
            }
            "decode_record_item" => {
                json!({"ok":true,"exit_code":0,"item_index":0,"create_time":100,"text":"合成记录条目"})
            }
            "decode_image" => {
                json!({"ok":true,"exit_code":0,"status":"published","image":{"format":"bmp","size":58}})
            }
            "decode_voice" | "transcribe_voice" => {
                json!({"ok":true,"prepared_audio":voice::prepared("synthetic","A",700)})
            }
            "decode_refer" => json!({"ok":true,"exit_code":0,"text":"合成引用回复",
                "username":"synthetic-a","local_id":i64::MAX,"create_time":100,
                "refer":{"reply_text":"合成回复正文","refer_sender":"合成发送者",
                    "refer_type":"1","refer_summary":"合成原文","refer_svrid":"18446744073709551615","refer_createtime":"99"}}),
            "contact_tags" => json!({"ok":true,"total_tags":1,"total_associations":2,
                "tags":[{"name":"合成标签","member_count":2}]}),
            "tag_members" => json!({"ok":true,"name":"合成标签","member_count":2,
                "members":[{"username":"synthetic-a","display_name":"合成甲"},{"username":"synthetic-b","display_name":"合成乙"}]}),
            "voice_messages" => json!({"ok":true,"count":2,"voices":[
                {"username":"synthetic-a","source":"message/media_0.db","chat_name_id":3,"media_rowid":7,
                    "local_id":91,"create_time":200,"voice_data_bytes":0},
                {"username":"synthetic-a","source":"message/media_1.db","chat_name_id":4,"media_rowid":8,
                    "local_id":92,"create_time":100,"voice_data_bytes":null}]}),
            _ => panic!("unregistered query reached IPC"),
        })
    });
    let server_b = Mock::start(b.runtime.clone(), |_| panic!("wrong account reached"));
    let original = fs::read(&a.config).unwrap();
    let output = fixture.temp.path().join("media-output");
    fs::create_dir(&output).unwrap();
    let mut command = fixture.command(Some(&a), None);
    command.arg("--media-output-root").arg(&output);
    let backend = tempfile::tempdir().unwrap();
    let model = voice::model(backend.path(), "A", 700, "合成语音识别");
    command
        .args([
            "--backend",
            "whisper_cpp",
            "--language",
            "zh",
            "--threads",
            "2",
        ])
        .arg("--whisper-binary")
        .arg(voice::executable())
        .arg("--whisper-model")
        .arg(&model)
        .arg("--temp-root")
        .arg(&output);
    let mut wx = Wx::start(command);
    wx.initialize();
    for (index, name) in TOOLS.iter().enumerate() {
        let args = match *name {
            "get_chat_history" | "get_chat_images" => json!({"chat_name":"synthetic"}),
            "search_messages" => json!({"keyword":"synthetic"}),
            "decode_transfer" | "decode_location" => json!({"chat_name":"synthetic","local_id":1}),
            "decode_refer" => json!({"chat_name":"synthetic","local_id":i64::MAX}),
            "decode_file_message" => json!({"chat_name":"synthetic","local_id":7}),
            "decode_image" => json!({"chat_name":"synthetic","local_id":9}),
            "decode_voice" | "transcribe_voice" => json!({"chat_name":"synthetic","local_id":700}),
            "decode_record_item" => {
                json!({"chat_name":"synthetic","local_id":8,"item_index":0,"create_time":100})
            }
            "get_tag_members" => json!({"tag_name":"合成标签"}),
            "get_voice_messages" => {
                json!({"chat_name":"synthetic","limit":2,"offset":3,"since":100,"until":200})
            }
            _ => json!({}),
        };
        let reply = wx.call(index as i64, name, args);
        assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
        if matches!(*name, "decode_voice" | "transcribe_voice") {
            let text = reply["result"]["content"][0]["text"].as_str().unwrap();
            assert_eq!(
                reply["result"],
                json!({"content":[{"type":"text","text":text}],"isError":false})
            );
            if *name == "decode_voice" {
                voice::assert_decode(text, "A", 700, &output);
            } else {
                voice::assert_transcribe(text, "合成语音识别", 700);
            }
            continue;
        }
        if index >= 8 {
            let data: Value =
                serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            match *name {
                "decode_image" => {
                    assert_eq!(
                        data,
                        json!({"exit_code":0,"status":"published","image":{"format":"bmp","size":58}})
                    );
                }
                "decode_file_message" => {
                    assert_eq!(data["exit_code"], 0);
                    assert_eq!(data["filename"], "合成文件.txt");
                    assert_eq!(data["size"], 12);
                }
                "decode_record_item" => {
                    assert_eq!(data["exit_code"], 0);
                    assert_eq!(data["item_index"], 0);
                    assert_eq!(data["create_time"], 100);
                    assert_eq!(data["text"], "合成记录条目");
                }
                "decode_refer" => {
                    assert_eq!(data["exit_code"], 0);
                    assert_eq!(data["local_id"], i64::MAX);
                    assert_eq!(data["refer"]["reply_text"], "合成回复正文");
                    assert_eq!(data["refer"]["refer_sender"], "合成发送者");
                    assert_eq!(data["refer"]["refer_summary"], "合成原文");
                    assert_eq!(data["refer"]["refer_svrid"], "18446744073709551615");
                }
                "get_contact_tags" => {
                    assert_eq!(data["total_tags"], 1);
                    assert_eq!(data["total_associations"], 2);
                    assert_eq!(data["tags"], json!([{"name":"合成标签","member_count":2}]));
                }
                "get_tag_members" => {
                    assert_eq!(data["name"], "合成标签");
                    assert_eq!(data["member_count"], 2);
                    assert_eq!(
                        data["members"],
                        json!([{"username":"synthetic-a","display_name":"合成甲"},{"username":"synthetic-b","display_name":"合成乙"}])
                    );
                }
                "get_voice_messages" => {
                    assert_eq!(data["count"], 2);
                    assert_eq!(data["voices"].as_array().unwrap().len(), 2);
                    assert_eq!(data["voices"][0]["local_id"], 91);
                    assert_eq!(data["voices"][0]["voice_data_bytes"], 0);
                    assert!(data["voices"][1]["voice_data_bytes"].is_null());
                }
                _ => unreachable!(),
            }
        }
        assert!(fs::OpenOptions::new().write(true).open(&a.config).is_ok());
    }
    for (index, name) in REQUIRED_VOICE_ARGS.iter().enumerate() {
        assert_eq!(
            wx.call(100 + index as i64, name, json!({}))["error"]["code"],
            -32602
        );
    }
    success(wx.finish());
    let requests = server_a.finish();
    let queries: Vec<_> = requests.iter().filter(|r| r["cmd"] != "ping").collect();
    assert_eq!(requests.len(), 34);
    assert_eq!(
        queries
            .iter()
            .map(|r| r["cmd"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "sessions",
            "contacts",
            "history",
            "search",
            "decode_transfer",
            "decode_location",
            "sessions",
            "attachments",
            "decode_refer",
            "contact_tags",
            "tag_members",
            "voice_messages",
            "decode_file_message",
            "decode_record_item",
            "decode_image",
            "decode_voice",
            "transcribe_voice"
        ]
    );
    assert_eq!(queries[6]["limit"], 10001);
    assert_eq!(queries[1], &json!({"cmd":"contacts","limit":50}));
    assert_eq!(queries[7]["kinds"], json!(["image"]));
    assert_eq!(
        queries[8],
        &json!({"cmd":"decode_refer","chat":"synthetic","local_id":i64::MAX,"create_time":0})
    );
    assert_eq!(queries[9], &json!({"cmd":"contact_tags"}));
    assert_eq!(
        queries[10],
        &json!({"cmd":"tag_members","tag_name":"合成标签"})
    );
    assert_eq!(
        queries[11],
        &json!({"cmd":"voice_messages","chat":"synthetic","limit":2,"offset":3,"since":100,"until":200})
    );
    assert_eq!(
        queries[12],
        &json!({"cmd":"decode_file_message","chat":"synthetic","local_id":7,"create_time":0})
    );
    assert_eq!(
        queries[13],
        &json!({"cmd":"decode_record_item","chat":"synthetic","local_id":8,"item_index":0,"create_time":100})
    );
    assert_eq!(
        queries[14],
        &json!({"cmd":"decode_image","chat":"synthetic","local_id":9,"create_time":0,"output_root":output})
    );
    assert_eq!(
        queries[15],
        &json!({"cmd":"decode_voice","chat":"synthetic","local_id":700})
    );
    assert_eq!(
        queries[16],
        &json!({"cmd":"transcribe_voice","chat":"synthetic","local_id":700})
    );
    assert!(server_b.finish().is_empty());
    assert_eq!(fs::read(&a.config).unwrap(), original);
    assert!(fs::OpenOptions::new().write(true).open(&a.config).is_ok());
    fixture.assert_no_daemon_or_database_output(&a);
    fixture.assert_no_daemon_or_database_output(&b);
}

#[test]
fn real_wx_bad_frames_notifications_and_eof_keep_stdout_protocol_only() {
    let fixture = Fixture::new();
    let mut wx = Wx::start(fixture.command(None, None));
    wx.raw(b"not-json\n\xff\n");
    for _ in 0..2 {
        assert_eq!(wx.reply()["error"]["code"], -32700);
    }
    wx.send(json!({"jsonrpc":"2.0","method":"ping"}));
    wx.raw(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"ping\"}\r\n");
    assert_eq!(wx.reply(), json!({"jsonrpc":"2.0","id":0,"result":{}}));
    success(wx.finish());
    for input in [b"SYNTHETIC_SECRET partial frame".to_vec(), vec![b'x'; 1025]] {
        let mut wx = Wx::start(fixture.command(None, Some(1024)));
        wx.raw(&input);
        let finished = wx.finish();
        assert!(!finished.status.success());
        assert!(finished.stdout_tail.is_empty());
        assert!(finished.stderr.contains("MCP stdio transport failed"));
        assert!(!finished.stderr.contains("SYNTHETIC_SECRET"));
    }
}

#[test]
fn real_wx_ipc_limit_is_inclusive_and_bad_backend_frames_are_safe() {
    let fixture = Fixture::new();
    let account = fixture.account("limits");
    let base = json!({"ok":true,"attachments":[],"padding":""}).to_string() + "\n";
    let padding = "p".repeat(1024 - base.len());
    let exact = format!(
        "{}\n",
        json!({"ok":true,"attachments":[],"padding":padding})
    )
    .into_bytes();
    assert_eq!(exact.len(), 1024);
    let mut next = 0;
    let server = Mock::start(account.runtime.clone(), move |_| {
        next += 1;
        match next {
            1 => Reply::Raw(exact.clone()),
            2 => Reply::Raw(vec![b'x'; 1025]),
            3 => Reply::Raw(b"SYNTHETIC_SECRET invalid-json\n".to_vec()),
            4 => Reply::Json(json!({"ok":false,"error":"SYNTHETIC_SECRET"})),
            _ => panic!("unexpected extra query"),
        }
    });
    let mut wx = Wx::start(fixture.command(Some(&account), Some(1024)));
    wx.initialize();
    assert_eq!(
        wx.call(1, "get_chat_images", json!({"chat_name":"synthetic"}))["result"]["isError"],
        false
    );
    for id in 2..=3 {
        tool_error(
            &wx.call(id, "get_chat_images", json!({"chat_name":"synthetic"})),
            "Query backend unavailable",
        );
    }
    // A valid business failure is distinct from an invalid transport frame,
    // while its private backend message must still never reach MCP output.
    let failure = wx.call(4, "get_chat_images", json!({"chat_name":"synthetic"}));
    tool_error(&failure, "Query failed");
    assert!(!failure.to_string().contains("SYNTHETIC_SECRET"));
    success(wx.finish());
    assert_eq!(server.finish().len(), 8);
    fixture.assert_no_daemon_or_database_output(&account);
}

#[test]
fn real_wx_response_expansion_limit_never_writes_partial_json() {
    let fixture = Fixture::new();
    let account = fixture.account("output-limit");
    let response = json!({"ok":true,"text":"\"".repeat(360)});
    assert!(response.to_string().len() + 1 < 1024);
    let server = Mock::start(account.runtime.clone(), move |_| {
        Reply::Json(response.clone())
    });
    let mut wx = Wx::start(fixture.command(Some(&account), Some(1024)));
    wx.initialize();
    wx.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_contacts"}}));
    let finished = wx.finish();
    assert!(!finished.status.success());
    assert!(finished.stdout_tail.is_empty());
    assert!(finished.stderr.contains("MCP stdio transport failed"));
    assert_eq!(server.finish().len(), 2);
    fixture.assert_no_daemon_or_database_output(&account);
}

#[test]
fn real_voice_response_budget_counts_request_id_before_wav_publication() {
    let fixture = Fixture::new();
    let account = fixture.account("voice-result-budget");
    let prepared = json!({"ok":true,"prepared_audio":voice::prepared("synthetic","A",700)});
    assert!(serde_json::to_vec(&prepared).unwrap().len() > 1024);
    let server = Mock::start(account.runtime.clone(), move |_| {
        Reply::Json(prepared.clone())
    });
    let output = tempfile::tempdir().unwrap();
    let mut command = fixture.command(Some(&account), Some(1024));
    command.arg("--media-output-root").arg(output.path());
    let mut wx = Wx::start(command);
    wx.initialize();
    let id = "x".repeat(800);
    let request = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"decode_voice","arguments":{"chat_name":"synthetic","local_id":700}}});
    assert!(serde_json::to_vec(&request).unwrap().len() < 1024);
    wx.send(request);
    let reply = wx.reply();
    assert_eq!(reply["id"], id);
    tool_error(&reply, "Query result exceeds safe limit");
    assert_eq!(
        fs::read_dir(output.path()).unwrap().count(),
        0,
        "budget failure published a WAV"
    );
    let success_reply = wx.call(
        2,
        "decode_voice",
        json!({"chat_name":"synthetic","local_id":700}),
    );
    assert_eq!(success_reply["result"]["isError"], false, "{success_reply}");
    voice::assert_decode(
        success_reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
        "A",
        700,
        output.path(),
    );
    success(wx.finish());
    assert_eq!(server.finish().len(), 4);
    fixture.assert_no_daemon_or_database_output(&account);
}

#[test]
fn real_wx_silent_ipc_times_out_and_closes_connection_without_worker_leak() {
    let fixture = Fixture::new();
    let account = fixture.account("silent");
    let closed = Arc::new(AtomicBool::new(false));
    let watch = closed.clone();
    let server = Mock::start(account.runtime.clone(), move |_| {
        Reply::UntilClientCloses(watch.clone())
    });
    let mut wx = Wx::start(fixture.command(Some(&account), None));
    wx.initialize();
    let started = Instant::now();
    let reply = wx.call(1, "get_contacts", json!({}));
    let elapsed = started.elapsed();
    tool_error(&reply, "Query timed out");
    // 普通send环境值为1秒，MCP应遵循真实context的30秒预算，允许调度抖动。
    assert!(
        elapsed >= Duration::from_secs(25) && elapsed < Duration::from_secs(38),
        "{elapsed:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while !closed.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        closed.load(Ordering::Acquire),
        "timed-out query left IPC connection open"
    );
    success(wx.finish());
    assert_eq!(server.finish().len(), 2);
    fixture.assert_no_daemon_or_database_output(&account);
}
