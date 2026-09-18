use super::{Account, Mcp};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn file_xml(name: &str, bytes: Option<&[u8]>) -> String {
    let hash = bytes
        .map(|b| format!("<md5>{:x}</md5>", md5::compute(b)))
        .unwrap_or_default();
    format!("<msg><appmsg><type>6</type><title>{name}</title>{hash}<appattach><fileext>txt</fileext></appattach></appmsg></msg>")
}

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

pub fn seed(conn: &Connection, table: &str, shard: usize, root: &Path, marker: &str) {
    let bytes = format!("raw-{marker}").into_bytes();
    let file = file_xml("report.txt", Some(&bytes));
    let insert = |id: i64, kind: i64, time: i64, body: &str| {
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,7,?4,0)"),
            params![id, kind, time, body],
        )
        .unwrap();
    };
    // 跨分片同一完整身份含非应用消息行：必须先判定歧义。
    insert(30, if shard == 0 { 49 } else { 1 }, 300, &file);
    if shard != 0 {
        return;
    }
    write(root, "msg/file/2026-09/report.txt", &bytes);
    for month in ["2026-08", "2026-09"] {
        write(
            root,
            &format!("msg/file/{month}/duplicate.txt"),
            b"duplicate",
        );
    }
    if marker == "B" {
        write(root, "msg/file/2026-09/only-b.txt", b"only-b");
    }
    insert(20, 49, 2000, &file);
    insert(21, 49, 2100, &file_xml("absent.txt", None));
    insert(22, 49, 2200, &file_xml("duplicate.txt", None));
    insert(23, 1, 2300, &file);
    insert(24, 49, 2400, &file_xml("only-b.txt", Some(b"only-b")));
    insert(25, 49, 2500, &file_xml("report.txt", Some(b"wrong")));
    insert(26, 49, 2600, &format!("{}{}", " ".repeat(20_001), file));
    insert(31, 49, 3100, &file);
    let hash = format!("{:x}", md5::compute(&bytes));
    let mut items = format!("<dataitem datatype=\"1\"><datadesc>text-{marker}</datadesc></dataitem><dataitem datatype=\"8\"><datatitle>record.txt</datatitle><fullmd5>{hash}</fullmd5></dataitem><dataitem datatype=\"2\"><fullmd5>{hash}</fullmd5></dataitem><dataitem datatype=\"4\"><datatitle>voice.raw</datatitle><fullmd5>{hash}</fullmd5></dataitem><dataitem datatype=\"5\"><datatitle>video.raw</datatitle><fullmd5>{hash}</fullmd5></dataitem><dataitem datatype=\"99\"/><dataitem datatype=\"8\"><datatitle>absent.txt</datatitle></dataitem>");
    for index in 7..61 {
        items.push_str(&format!(
            "<dataitem datatype=\"1\"><datadesc>item-{marker}-{index} {}</datadesc></dataitem>",
            "x".repeat(400)
        ));
    }
    let record = format!("<msg><appmsg><type>19</type><recorditem><![CDATA[<recordinfo><datalist>{items}</datalist></recordinfo>]]></recorditem></appmsg></msg>");
    assert!(record.len() > 20_000 && record.len() < 500_000);
    insert(40, 49, 4000, &record);
    insert(41, 49, 4100, "<msg><appmsg><type>19</type></appmsg></msg>");
    let rec = format!("msg/attach/{:x}/2026-09/Rec/card", md5::compute(b"peer"));
    for suffix in [
        "F/1/record.txt",
        "Img/2.dat",
        "A/3/voice.raw",
        "V/4/video.raw",
    ] {
        write(root, &format!("{rec}/{suffix}"), &bytes);
    }
}

fn args(id: i64, index: Option<i64>) -> Value {
    let mut args = json!({"chat_name":"peer","local_id":id});
    if let Some(index) = index {
        args["item_index"] = json!(index);
    }
    args
}

pub fn verify_found(
    account: &Account,
    mcp: &mut Mcp,
    tool: &str,
    id: i64,
    index: Option<i64>,
    name: &str,
    kind: &str,
) {
    let result = mcp.data(tool, args(id, index));
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["status"], "found");
    assert_eq!(result["metadata"]["kind"], kind);
    assert_eq!(
        result["metadata"]["identity"],
        json!({"username":"peer","source":"message/message_0.db","local_id":id,"create_time":id*100})
    );
    assert_eq!(result["metadata"]["item_index"], json!(index));
    let path = Path::new(result["reference"]["path"].as_str().unwrap());
    assert!(path.starts_with(account.root()), "{result}");
    assert_eq!(path.file_name().unwrap(), name);
    let bytes = format!("raw-{}", account.marker).into_bytes();
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(result["reference"]["size"], bytes.len());
    assert_eq!(
        result["reference"]["md5"],
        format!("{:x}", md5::compute(&bytes))
    );
    assert_eq!(result["reference"]["binding"], "md5");
    assert_eq!(result["reference"]["warning"], Value::Null);
}

pub fn verify(account: &Account, mcp: &mut Mcp) {
    verify_found(
        account,
        mcp,
        "decode_file_message",
        20,
        None,
        "report.txt",
        "file",
    );
    for (index, name, kind) in [
        (1, "record.txt", "file"),
        (2, "2.dat", "image"),
        (3, "voice.raw", "voice"),
        (4, "video.raw", "video"),
    ] {
        verify_found(
            account,
            mcp,
            "decode_record_item",
            40,
            Some(index),
            name,
            kind,
        );
    }
    for (tool, id, index, status) in [
        ("decode_file_message", 21, None, "missing"),
        ("decode_record_item", 40, Some(0), "text"),
        ("decode_record_item", 40, Some(5), "metadata_only"),
        ("decode_record_item", 40, Some(6), "missing"),
        ("decode_record_item", 40, Some(60), "text"),
    ] {
        let result = mcp.data(tool, args(id, index));
        assert_eq!(result["status"], status);
        assert_eq!(result["reference"], Value::Null);
        if index.is_some() {
            assert_eq!(result["metadata"]["item_count"], 61);
        }
        if index == Some(60) {
            assert!(result["metadata"]["description"]
                .as_str()
                .unwrap()
                .starts_with(&format!("item-{}-60 ", account.marker)));
        }
    }
    let other = mcp.data("decode_file_message", args(24, None));
    assert_eq!(
        other["status"],
        if account.marker == "A" {
            "missing"
        } else {
            "found"
        }
    );
    if account.marker == "A" {
        assert_eq!(other["reference"], Value::Null);
    }
    for (tool, id, index, code) in [
        ("decode_file_message", 22, None, 2),
        ("decode_file_message", 23, None, 1),
        ("decode_file_message", 25, None, 1),
        ("decode_file_message", 30, None, 2),
        ("decode_record_item", 30, Some(999), 2),
        ("decode_file_message", 41, None, 1),
        ("decode_record_item", 31, Some(0), 1),
        ("decode_record_item", 40, Some(61), 1),
        ("decode_file_message", 999, None, 1),
    ] {
        let mut request = json!({"cmd":tool,"chat":"peer","local_id":id,"create_time":0});
        if let Some(index) = index {
            request["item_index"] = json!(index);
        }
        let raw = account.ipc(request).unwrap();
        assert_eq!(raw["ok"], true, "{raw}");
        assert_eq!(raw["exit_code"], code, "{raw}");
        assert!(raw.get("reference").is_none(), "{raw}");
        let expected = if code == 2 {
            assert_eq!(raw["status"], "ambiguous", "{raw}");
            assert_eq!(raw["error_code"], "ambiguous_identity", "{raw}");
            "Business request refused"
        } else {
            "Query failed"
        };
        crate::safe_failure(mcp.call(tool, args(id, index)), expected);
    }
    crate::safe_failure(
        mcp.call("decode_file_message", args(26, None)),
        "Query failed",
    );
    let mut invalid = args(20, None);
    invalid["base"] = json!(account.root());
    assert_eq!(
        mcp.call("decode_file_message", invalid)["error"]["code"],
        -32602
    );
    assert_eq!(
        mcp.call("decode_record_item", args(40, Some(-1)))["error"]["code"],
        -32602
    );
}
