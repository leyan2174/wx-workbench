use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
};
use wx_mcp_cli_harness::authenticated_mock::{Mock, Reply};
use wx_mcp_cli_harness::ipc::{Request, Response};
pub use wx_mcp_cli_harness::{ipc, mcp, mcp_service, runtime};

fn command(config: Option<&Path>, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcp-cli-harness"));
    cmd.env_remove("WX_DAEMON_MODE")
        .env_remove("WX_CLI_EXPECTED_RUNTIME")
        .env_remove("WX_CLI_CONFIG")
        .env("WX_CLI_HOME", home)
        .env("WX_CLI_REQUEST_TIMEOUT_SECS", "2");
    if let Some(config) = config {
        cmd.env("WX_CLI_CONFIG", config);
    }
    cmd
}

fn account(base: &Path, name: &str) -> (PathBuf, PathBuf, String) {
    let config = base.join(format!("{name}.json"));
    let home = base.join(format!("{name}-home"));
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "db_dir":base.join(format!("{name}-missing-databases")),
            "keys_file":base.join(format!("{name}-missing-keys.json")),
            "decrypted_dir":base.join(format!("{name}-missing-decrypted")),
            "wechat_process":"synthetic-not-a-process.exe"
        }))
        .unwrap(),
    )
    .unwrap();
    let output = command(Some(&config), &home)
        .arg("--fixture-pipe")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let pipe = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert!(pipe.starts_with("wx-cli-v2-"));
    (config, home, pipe)
}

fn mock(config: &Path, home: &Path) -> Mock {
    let runtime = wx_mcp_cli_harness::fixture_runtime(config, home).unwrap();
    Mock::start(runtime, |value| {
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        let response = match &request {
            Request::Ping => Response::ok(json!({"pong":true})),
            Request::Sessions { .. } => Response::ok(json!({"sessions":[]})),
            Request::Contacts(args) if args.query.as_deref() == Some("synthetic-error") => {
                Response::err("PRIVATE_MESSAGE SYNTHETIC_KEYS")
            }
            Request::Contacts(_) => Response::ok(
                json!({"contacts":[{"username":"synthetic","display":"synthetic"}],"total":1}),
            ),
            Request::History { .. } => Response::ok(json!({"messages":[],"count":0})),
            Request::Search { .. } => Response::ok(json!({"results":[],"count":0})),
            Request::DecodeTransfer { .. } | Request::DecodeLocation { .. } => {
                Response::ok(json!({"exit_code":0,"text":"synthetic decoded metadata"}))
            }
            Request::Attachments { .. } => Response::ok(json!({"attachments":[],"count":0})),
            _ => panic!("unexpected non-query request"),
        };

        Reply::Json(serde_json::to_value(response).unwrap())
    })
}

fn send(input: &mut ChildStdin, frame: Value) {
    writeln!(input, "{frame}").unwrap();
}

fn read_reply(output: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert!(
        output.read_line(&mut line).unwrap() > 0,
        "missing protocol reply"
    );
    let reply: Value = serde_json::from_str(&line).expect("stdout must contain JSON only");
    assert_eq!(reply["jsonrpc"], "2.0");
    reply
}

fn initialize(input: &mut ChildStdin, output: &mut impl BufRead) {
    send(
        input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}),
    );
    assert_eq!(
        read_reply(output)["result"]["protocolVersion"],
        "2025-06-18"
    );
    send(
        input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
}

#[test]
fn eight_tools_use_real_account_isolated_transport_without_database_access() {
    let temp = tempfile::tempdir().unwrap();
    let (config_a, home_a, pipe_a) = account(temp.path(), "a");
    let (config_b, home_b, pipe_b) = account(temp.path(), "b");
    assert_ne!(pipe_a, pipe_b);
    let server_a = mock(&config_a, &home_a);
    let server_b = mock(&config_b, &home_b);
    let original = std::fs::read(&config_a).unwrap();
    let mut child = command(Some(&config_a), &home_a)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    initialize(&mut input, &mut output);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let list = read_reply(&mut output);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 23);
    let calls = [
        ("get_recent_sessions", json!({})),
        ("get_contacts", json!({})),
        ("get_chat_history", json!({"chat_name":"synthetic"})),
        ("search_messages", json!({"keyword":"synthetic"})),
        (
            "decode_transfer",
            json!({"chat_name":"synthetic","local_id":1}),
        ),
        (
            "decode_location",
            json!({"chat_name":"synthetic","local_id":1}),
        ),
        ("get_new_messages", json!({})),
        ("get_chat_images", json!({"chat_name":"synthetic"})),
    ];
    for (index, (name, arguments)) in calls.into_iter().enumerate() {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":10+index,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        );
        let reply = read_reply(&mut output);
        assert_eq!(reply["id"], 10 + index);
        assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&config_a)
                .is_ok(),
            "idle session must release the operation-scoped configuration lock"
        );
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":90,"method":"tools/call","params":{"name":"transcribe_voice","arguments":{}}}),
    );
    assert_eq!(read_reply(&mut output)["error"]["code"], -32602);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"get_contacts","arguments":{"query":"synthetic-error"}}}),
    );
    let failure = read_reply(&mut output);
    assert_eq!(failure["result"]["isError"], true);
    assert!(!failure.to_string().contains("PRIVATE_MESSAGE"));
    assert!(!failure.to_string().contains("SYNTHETIC_KEYS"));
    drop(input);
    let mut tail = String::new();
    std::io::Read::read_to_string(&mut output, &mut tail).unwrap();
    assert!(tail.is_empty(), "extra stdout after protocol EOF");
    let exited = child.wait_with_output().unwrap();
    assert!(exited.status.success());
    assert!(
        exited.stderr.is_empty(),
        "no daemon startup or private errors expected"
    );
    let received: Vec<Request> = server_a
        .finish()
        .into_iter()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    assert_eq!(received.len(), 18);
    assert_eq!(
        received
            .iter()
            .filter(|r| matches!(r, Request::Ping))
            .count(),
        9
    );
    assert!(server_b.finish().is_empty());
    assert_eq!(std::fs::read(&config_a).unwrap(), original);
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&config_a)
            .is_ok(),
        "lock released at EOF"
    );
    for (config, home) in [(&config_a, &home_a), (&config_b, &home_b)] {
        let runtime = wx_mcp_cli_harness::fixture_runtime(config, home).unwrap();
        let names: Vec<_> = std::fs::read_dir(runtime.directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names.is_empty(),
            "mock identity files must be removed at shutdown"
        );
    }
}

#[test]
fn absent_explicit_account_still_allows_handshake_but_never_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("unconfigured-home");
    let mut child = command(None, &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    initialize(&mut input, &mut output);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_contacts"}}),
    );
    let reply = read_reply(&mut output);
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(
        reply["result"]["content"][0]["text"],
        "Query backend unavailable"
    );
    drop(input);
    let exited = child.wait_with_output().unwrap();
    assert!(exited.status.success());
    assert!(exited.stderr.is_empty());
    assert!(!home.exists());
}

#[cfg(windows)]
fn lock_unreadable(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
fn handshake_only(cmd: &mut Command) {
    println!("COMMAND: {cmd:?}");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    initialize(&mut input, &mut output);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let reply = read_reply(&mut output);
    println!("STDOUT tools/list: {reply}");
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 23);
    for name in ["decode_voice", "transcribe_voice"] {
        assert!(!tools.iter().any(|tool| tool["name"] == name));
    }
    drop(input);
    let mut tail = String::new();
    std::io::Read::read_to_string(&mut output, &mut tail).unwrap();
    assert!(tail.is_empty());
    let exited = child.wait_with_output().unwrap();
    println!("STDERR: {}", String::from_utf8_lossy(&exited.stderr));
    println!("EXIT: {}", exited.status);
    assert!(exited.status.success());
    assert!(exited.stderr.is_empty());
}

#[cfg(windows)]
#[test]
fn handshake_does_not_open_image_keys_or_prepare_output_directories() {
    let temp = tempfile::tempdir().unwrap();
    let key = temp.path().join("image-key.json");
    std::fs::write(&key, b"synthetic-invalid-key").unwrap();
    let locked = lock_unreadable(&key);
    assert!(std::fs::read(&key).is_err());
    let home = temp.path().join("missing-home");
    let output = temp.path().join("missing-output");
    let mut cmd = command(None, &home);
    cmd.arg("--image-key-file")
        .arg(&key)
        .arg("--media-output-root")
        .arg(&output);
    handshake_only(&mut cmd);
    drop(locked);
    assert_eq!(std::fs::read(key).unwrap(), b"synthetic-invalid-key");
    assert!(!home.exists());
    assert!(!output.exists());
}

#[test]
fn removed_voice_host_options_are_cli_errors_without_creating_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("missing-home");
    for option in [
        "--backend",
        "--whisper-binary",
        "--whisper-model",
        "--voice-cache-file",
        "--configured-local-python",
        "--allow-upload",
    ] {
        let output = command(None, &home).arg(option).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{option}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(option));
        assert!(!home.exists());
    }
}

#[test]
fn historical_task_and_step_kinds_fail_deserialization_explicitly() {
    use wx_mcp_cli_harness::service::{plan::Step, protocol::Kind};
    for kind in [
        "transcribe",
        "transcribe_chats",
        "voice_export",
        "voice_mp3",
    ] {
        let error = serde_json::from_value::<Kind>(json!(kind)).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
        assert!(error.to_string().contains(kind), "{error}");
    }
    for operation in [
        "transcribe",
        "transcribe_chats",
        "voice_export",
        "voice_batch",
    ] {
        let error = serde_json::from_value::<Step>(json!({
            "operation":operation, "config":"synthetic.json", "output":"synthetic", "users":[], "allow_upload":false
        })).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
        assert!(error.to_string().contains(operation), "{error}");
    }
}
