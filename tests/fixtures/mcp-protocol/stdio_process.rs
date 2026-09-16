use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn actual_process_stdout_contains_only_responses() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_synthetic-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"get_contacts"}}),
        json!({"jsonrpc":"2.0","id":"query","method":"tools/call","params":{"name":"get_contacts"}}),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_contacts","arguments":{"query":"synthetic-error"}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for frame in frames {
        writeln!(input, "{frame}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let replies: Vec<Value> = stdout
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(replies.len(), 4);
    assert!(replies
        .iter()
        .all(|v| v["jsonrpc"] == "2.0" && v.get("result").is_some()));
    assert_eq!(replies[1]["id"], "query");
    let data: Value =
        serde_json::from_str(replies[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["request"], json!({"cmd":"contacts","limit":50}));
    assert_eq!(data["text"], "first\nsecond");
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!(replies[3]["result"]["content"][0]["text"], "Query failed");
    assert!(!stdout.contains("SYNTHETIC_PRIVATE"));
    assert!(!stdout.contains("SYNTHETIC_SECRET"));
}
