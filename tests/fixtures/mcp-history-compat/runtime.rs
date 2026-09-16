//! 不替代查询或渲染：复用账号夹具，通过实际 wx daemon IPC 验证新旧路径。
#![cfg(windows)]
#[path = "../../../src/private_file.rs"]
#[allow(dead_code)] // Shared production module; ACL inspection runs in the root security tests.
pub(crate) mod private_file;

#[path = "../mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;
use serde_json::{json, Value};

pub(crate) fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}

fn history(account: &support::Account, extra: Value) -> Value {
    let mut request = json!({"cmd":"history","chat":"peer","limit":1000,"offset":0});
    request
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    account.ipc(request).unwrap()
}

#[test]
fn real_q_history_keeps_mapper_and_selects_globally_before_paging() {
    let home = tempfile::tempdir().unwrap();
    let mut account = support::Account::new(home.path(), "A");
    let before = account.snapshot();
    account.start();
    let baseline = history(&account, json!({}));
    assert_eq!(baseline["ok"], true, "{baseline}");
    let all = baseline["messages"].as_array().unwrap();
    assert!(all.len() > 10);
    // 两条 local_id=8 来自不同分片，必须同时保留，不能按 local_id 去重。
    assert_eq!(all.iter().filter(|m| m["local_id"] == 8).count(), 2);
    let single = history(&account, json!({"msg_type":49}));
    assert_eq!(single["ok"], true);
    let empty = history(&account, json!({"msg_type":49,"msg_types":[]}));
    assert_eq!(empty, single);
    let list = history(&account, json!({"msg_types":[49]}));
    assert_eq!(list["messages"], single["messages"]);
    for oldest in [false, true] {
        for (since, until) in [(100, 4100), (110, 3100), (300, 300)] {
            for (limit, offset) in [(3, 1), (2, 0), (1, 100)] {
                let mut expected: Vec<_> = all
                    .iter()
                    .filter(|m| {
                        let timestamp = m["timestamp"].as_i64().unwrap();
                        (since..=until).contains(&timestamp)
                    })
                    .cloned()
                    .collect();
                expected.sort_by(|a, b| {
                    let a = a["timestamp"].as_i64().unwrap();
                    let b = b["timestamp"].as_i64().unwrap();
                    if oldest {
                        a.cmp(&b)
                    } else {
                        b.cmp(&a)
                    }
                });
                let mut expected: Vec<_> = expected.into_iter().skip(offset).take(limit).collect();
                expected.sort_by_key(|m| m["timestamp"].as_i64().unwrap());
                let value = history(
                    &account,
                    json!({"msg_types":[1,49],"oldest_first":oldest,
                    "since":since,"until":until,"limit":limit,"offset":offset}),
                );
                assert_eq!(value["ok"], true, "{value}");
                assert_eq!(
                    value["messages"],
                    json!(expected),
                    "oldest={oldest}, {since}..{until}, limit={limit}, offset={offset}"
                );
                assert_eq!(value["count"], expected.len());
            }
        }
    }
    let filtered_oldest = history(
        &account,
        json!({"msg_types":[49],"oldest_first":true,"limit":1}),
    );
    assert_eq!(filtered_oldest["messages"][0], single["messages"][0]);
    assert_eq!(history(&account, json!({"msg_types":[-1]}))["count"], 0);
    for (extra, message) in [
        (
            json!({"msg_type":49,"msg_types":[49]}),
            "conflicting history",
        ),
        (
            json!({"msg_type":49,"msg_types":[1,49]}),
            "conflicting history",
        ),
        (json!({"msg_types":vec![49;101]}), "too many history types"),
        (json!({"limit":0}), "history limit"),
        (json!({"offset":i64::MAX,"limit":1}), "SQLite integer range"),
        (json!({"since":2,"until":1}), "time range"),
    ] {
        let mut extra = extra;
        // 未知联系人仍先返回参数错误，证明失败先于账号/分片查询。
        extra["chat"] = json!("missing-contact");
        let value = history(&account, extra);
        assert_eq!(value["ok"], false, "{value}");
        assert!(value.to_string().contains(message), "{value}");
        assert!(value.get("messages").is_none(), "{value}");
    }
    assert_eq!(account.snapshot(), before);
}
