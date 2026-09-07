//! 真实账号跨分片历史查询；页边界与类型并集不能靠反转旧页实现。
use super::{Account, Mcp};
use chrono::{Local, TimeZone};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn seed(conn: &Connection, table: &str, shard: usize, marker: &str) {
    for (index, kind) in [1_i64, 3, 34, 1, 3, (1 << 32) | 1, 49, 1]
        .into_iter()
        .enumerate()
    {
        if index % 2 != shard {
            continue;
        }
        let id = 101 + index as i64;
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,7,?4,0)"),
            params![
                id,
                kind,
                10_001 + index as i64,
                format!("history-{marker}-{id}")
            ],
        )
        .unwrap();
    }
}

fn ids(value: &Value) -> Vec<i64> {
    value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["local_id"].as_i64().unwrap())
        .collect()
}

pub fn verify(account: &Account, mcp: &mut Mcp) {
    let time = |timestamp| {
        Local
            .timestamp_opt(timestamp, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    };
    let start = time(10_001);
    let end = time(10_008);
    let mut command = account.command();
    command.args([
        "history",
        "peer",
        "--types",
        "text,image",
        "--oldest-first",
        "--since",
        &start,
        "--until",
        &end,
        "--offset",
        "1",
        "--limit",
        "2",
        "--json",
    ]);
    println!("HISTORY CLI COMMAND: {command:?}");
    let output = command.output().unwrap();
    println!(
        "HISTORY CLI STDOUT: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    println!(
        "HISTORY CLI STDERR: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    assert_eq!(
        ids(&serde_json::from_slice(&output.stdout).unwrap()),
        vec![102, 104]
    );
    for (types, oldest, offset, expected) in [
        (json!(["text", "image"]), true, 1, vec![102, 104]),
        (json!(["text", "image"]), false, 1, vec![105, 106]),
        (json!(["text"]), true, 1, vec![104, 106]),
        (json!([]), true, 0, vec![101, 102]),
        (json!(null), false, 0, vec![107, 108]),
    ] {
        let result = mcp.data(
            "get_chat_history",
            json!({
                "chat_name":"peer","since":10_001,"until":10_008,
                "msg_types":types,"oldest_first":oldest,"offset":offset,"limit":2,
            }),
        );
        assert_eq!(ids(&result), expected, "{result}");
        assert_eq!(result["count"], 2);
        for row in result["messages"].as_array().unwrap() {
            if [101, 104, 106, 108].contains(&row["local_id"].as_i64().unwrap()) {
                assert!(
                    row["content"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("history-{}-", account.marker)),
                    "{row}"
                );
            }
        }
    }
    let bounded = mcp.data(
        "get_chat_history",
        json!({
            "chat_name":"peer","msg_types":["text","image"],"oldest_first":true,
            "since":10_004,"until":10_006,"limit":20,
        }),
    );
    assert_eq!(ids(&bounded), vec![104, 105, 106]);
    let old = account
        .ipc(json!({"cmd":"history","chat":"peer","since":10_001,"until":10_008,"limit":2}))
        .unwrap();
    assert_eq!(ids(&old), vec![107, 108]);
    let conflict = account
        .ipc(json!({"cmd":"history","chat":"does-not-exist","msg_type":1,"msg_types":[1]}))
        .unwrap();
    assert_eq!(conflict["ok"], false, "{conflict}");
    assert!(conflict["error"].as_str().unwrap().contains("conflicting"));
}
