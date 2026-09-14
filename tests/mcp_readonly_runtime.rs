//! 六个只读工具：真实合成加密库 -> 真实 daemon -> 真实 wx mcp。
#![cfg(windows)]
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
mod support;
use serde_json::{json, Value};
use support::{Account, Mcp};

const TOOLS: [&str; 17] = [
    "get_recent_sessions",
    "get_contacts",
    "get_chat_history",
    "search_messages",
    "decode_transfer",
    "decode_location",
    "get_new_messages",
    "get_chat_images",
    "get_contact_tags",
    "get_tag_members",
    "decode_refer",
    "get_voice_messages",
    "decode_file_message",
    "decode_record_item",
    "decode_image",
    "decode_voice",
    "transcribe_voice",
];
const REQUIRED_VOICE_ARGS: [&str; 2] = ["decode_voice", "transcribe_voice"];

fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}

fn tags(mcp: &mut Mcp, marker: &str, count: usize) {
    // 完整对象相等也保证列表没有 members、用户名或其他未授权字段。
    assert_eq!(
        mcp.data("get_contact_tags", json!({})),
        json!({
            "total_tags":2,"total_associations":count,
            "tags":[{"name":format!("标签{marker}"),"member_count":count},{"name":"","member_count":0}]
        })
    );
    assert_eq!(
        mcp.data(
            "get_tag_members",
            json!({"tag_name":format!("标签{marker}")})
        ),
        json!({
            "name":format!("标签{marker}"),"member_count":count,
            "members":vec![json!({"username":"peer","display_name":format!("姓名{marker}")});count]
        })
    );
}

fn refer(mcp: &mut Mcp, marker: &str, id: i64, timestamp: i64) {
    let decoded = mcp.data(
        "decode_refer",
        json!({"chat_name":"peer","local_id":id,"create_time":timestamp}),
    );
    assert_eq!(decoded["exit_code"], 0);
    assert_eq!(decoded["username"], "peer");
    assert_eq!(decoded["local_id"], id);
    assert_eq!(decoded["create_time"], timestamp);
    assert_eq!(
        decoded["source"],
        if timestamp == 201 {
            "message/message_1.db"
        } else {
            "message/message_0.db"
        }
    );
    assert_eq!(
        decoded["refer"],
        json!({
            "reply_text":format!("reply-{marker}-{timestamp}"),"refer_sender":"peer",
            "refer_type":"1","refer_type_label":"文本","refer_summary":format!("original-{marker}"),
            "refer_svrid":"700","refer_createtime":"90","refer_fromusr":"peer",
            "refer_chatusr":"peer","refer_displayname":"original-sender"
        })
    );
    assert!(decoded["text"]
        .as_str()
        .unwrap()
        .contains(&format!("reply-{marker}-{timestamp}")));
}

#[test]
fn six_readonly_tools_use_real_encrypted_accounts_and_reject_missing_voice_arguments() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    let mut b = Account::new(home.path(), "B");
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.start();
    b.start();
    let mut ma = a.mcp();
    let mut mb = b.mcp();
    ma.ready();
    mb.ready();
    for mcp in [&mut ma, &mut mb] {
        let list = mcp.rpc("tools/list", json!({}));
        let mut names: Vec<_> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        let mut expected = TOOLS.to_vec();
        names.sort();
        expected.sort();
        assert_eq!(names, expected);
        for name in REQUIRED_VOICE_ARGS {
            let rejected = mcp.call(name, json!({}));
            assert_eq!(rejected["error"]["code"], -32602, "{rejected}");
            assert!(rejected.get("result").is_none());
        }
    }
    tags(&mut ma, "A", 2);
    tags(&mut mb, "B", 1);
    safe_failure(
        ma.call("get_tag_members", json!({"tag_name":"标签B"})),
        "Query failed",
    );
    safe_failure(
        mb.call("get_tag_members", json!({"tag_name":"标签A"})),
        "Query failed",
    );
    for (account, mcp, length) in [(&a, &mut ma, 3), (&b, &mut mb, 5)] {
        refer(mcp, account.marker, 7, 100);
        // 底层真实 IPC 明确产生业务 exit_code=2；不是 mock 或任意后端失败。
        let ambiguous = account
            .ipc(json!({"cmd":"decode_refer","chat":"peer","local_id":8,"create_time":0}))
            .unwrap();
        assert_eq!(ambiguous["ok"], true);
        assert_eq!(ambiguous["exit_code"], 2);
        assert!(ambiguous.get("refer").is_none());
        safe_failure(
            mcp.call("decode_refer", json!({"chat_name":"peer","local_id":8})),
            "Query failed",
        );
        // 时间筛选后两条都能恢复成功，避免用损坏库伪造歧义通过。
        refer(mcp, account.marker, 8, 200);
        refer(mcp, account.marker, 8, 201);
        let voice = |rowid, id, timestamp, size: Value| json!({"username":"peer","source":"message/media_0.db","chat_name_id":7,"media_rowid":rowid,"local_id":id,"create_time":timestamp,"voice_data_bytes":size});
        let expected = vec![
            voice(2, 8, 110, json!(length)),
            voice(1, 7, 100, Value::Null),
        ];
        assert_eq!(
            mcp.data("get_voice_messages", json!({"chat_name":"peer"})),
            json!({"voices":expected,"count":2})
        );
        assert_eq!(
            mcp.data(
                "get_voice_messages",
                json!({"chat_name":"peer","since":100,"until":100})
            ),
            json!({"voices":[voice(1,7,100,Value::Null)],"count":1})
        );
        assert_eq!(
            mcp.data(
                "get_voice_messages",
                json!({"chat_name":"peer","limit":1,"offset":1})
            ),
            json!({"voices":[voice(1,7,100,Value::Null)],"count":1})
        );
    }
    // 再次交错读取，防止首次成功后被第二账号的缓存污染。
    support::attachments::verify(&a, &mut ma);
    support::attachments::verify(&b, &mut mb);
    support::history::verify(&a, &mut ma);
    support::history::verify(&b, &mut mb);
    support::attachments::verify_found(
        &a,
        &mut ma,
        "decode_file_message",
        20,
        None,
        "report.txt",
        "file",
    );
    tags(&mut ma, "A", 2);
    refer(&mut mb, "B", 7, 100);
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
    ma.finish();
    mb.finish();
}
