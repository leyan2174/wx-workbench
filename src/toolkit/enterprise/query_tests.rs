use super::*;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

const GOLDEN: &str = include_str!("../../../tests/fixtures/enterprise/queries/golden.json");
const USER: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/queries/user.db");
const SESSION: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/queries/session.db");
const MESSAGES: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/queries/message.db");
const SELF_ID: i64 = 10000000001;
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "enterprise-query-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::create_dir(p.join("input")).unwrap();
        for (name, bytes) in [
            ("user.db", USER),
            ("session.db", SESSION),
            ("message.db", MESSAGES),
        ] {
            fs::write(p.join("input").join(name), bytes).unwrap();
        }
        Self(p)
    }
    fn store(&self) -> OfflineStore {
        OfflineStore::open(&self.0.join("input"), Some(SELF_ID)).unwrap()
    }
    fn unchanged(&self) {
        for (name, bytes) in [
            ("user.db", USER),
            ("session.db", SESSION),
            ("message.db", MESSAGES),
        ] {
            assert_eq!(fs::read(self.0.join("input").join(name)).unwrap(), bytes);
        }
        assert_eq!(fs::read_dir(self.0.join("input")).unwrap().count(), 3);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn normalized(messages: &[Message]) -> Value {
    let mut value = serde_json::to_value(messages).unwrap();
    for row in value.as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("time");
    }
    value
}

#[test]
fn fixture_hashes_match_the_recorded_synthetic_oracle() {
    use sha2::{Digest, Sha256};
    let oracle: Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(oracle["synthetic_only"], true);
    for (name, bytes) in [
        ("user.db", USER),
        ("session.db", SESSION),
        ("message.db", MESSAGES),
    ] {
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hash, oracle["sha256"][name].as_str().unwrap());
    }
}

#[test]
fn matches_vendor_contacts_conversations_and_message_semantics() {
    let f = Fixture::new();
    let db = f.store();
    let oracle: Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(
        serde_json::to_value(db.contacts().unwrap()).unwrap(),
        oracle["contacts"]
    );
    assert_eq!(
        serde_json::to_value(db.conversations().unwrap()).unwrap(),
        oracle["conversations"]
    );
    assert_eq!(
        normalized(&db.messages(&MessageFilter::default()).unwrap()),
        oracle["messages"]
    );
    f.unchanged();
}

#[test]
fn content_decode_matches_vendor_and_rejects_numeric_schema() {
    let oracle: Value = serde_json::from_str(GOLDEN).unwrap();
    for case in oracle["decode_cases"].as_array().unwrap() {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(
            content::decode_bytes(&bytes),
            case["text"].as_str().unwrap(),
            "case {hex}"
        );
    }
    assert!(content::decode_value(rusqlite::types::ValueRef::Integer(1)).is_err());
    assert_eq!(content::format_time(0), "");
    assert_eq!(content::format_time(-1), "");
    assert_eq!(
        content::format_time(1700000006),
        content::format_time(1700000006000)
    );
}

#[test]
fn filtering_is_bound_global_and_after_deduplication() {
    let f = Fixture::new();
    let db = f.store();
    let group = MessageFilter {
        conversation_ids: vec!["R:team".into()],
        ..Default::default()
    };
    let messages = db.messages(&group).unwrap();
    assert_eq!(
        messages.iter().map(|m| m.message_id).collect::<Vec<_>>(),
        vec![1, 2, 5, 6]
    );
    assert_eq!(
        db.messages(&MessageFilter {
            offset: 1,
            limit: Some(2),
            ..group.clone()
        })
        .unwrap(),
        messages[1..3]
    );
    assert!(db
        .messages(&MessageFilter {
            limit: Some(0),
            ..group.clone()
        })
        .unwrap()
        .is_empty());
    let peer = db
        .messages(&MessageFilter {
            sender_id: Some(10000000002),
            ..group.clone()
        })
        .unwrap();
    assert_eq!(
        peer.iter().map(|m| m.message_id).collect::<Vec<_>>(),
        vec![2, 5, 6]
    );
    assert_eq!(
        db.messages(&MessageFilter {
            start_time: Some(1700000001),
            end_time: Some(1700000004),
            ..group.clone()
        })
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        db.messages(&MessageFilter {
            contains: Some("回退".into()),
            ..group.clone()
        })
        .unwrap()[0]
            .message_id,
        6
    );
    assert_eq!(
        db.messages(&MessageFilter {
            content_type: Some(7),
            ..group
        })
        .unwrap()[0]
            .message_id,
        5
    );
    assert!(db
        .messages(&MessageFilter {
            conversation_ids: vec!["' OR 1=1 --".into()],
            ..Default::default()
        })
        .unwrap()
        .is_empty());
    assert!(db
        .messages(&MessageFilter {
            start_time: Some(2),
            end_time: Some(1),
            ..Default::default()
        })
        .is_err());
}

#[test]
fn export_json_csv_html_is_safe_complete_and_non_destructive() {
    let f = Fixture::new();
    let db = f.store();
    for (name, format) in [
        ("result.json", ExportFormat::Json),
        ("result.csv", ExportFormat::Csv),
        ("result.html", ExportFormat::Html),
    ] {
        let path = f.0.join(name);
        let report = db.export_conversation("R:team", &path, format).unwrap();
        assert_eq!(report.message_count, 4);
        let bytes = fs::read(&path).unwrap();
        assert!(db.export_conversation("R:team", &path, format).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let json: Value = serde_json::from_slice(&fs::read(f.0.join("result.json")).unwrap()).unwrap();
    assert_eq!(json["message_count"], 4);
    assert!(json["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("=1+2"));
    let csv = fs::read(f.0.join("result.csv")).unwrap();
    assert!(csv.starts_with(b"\xef\xbb\xbf"));
    let mut reader = csv::Reader::from_reader(&csv[3..]);
    let records: Vec<_> = reader.records().map(|r| r.unwrap()).collect();
    assert_eq!(records.len(), 4);
    assert!(records[0][6].starts_with("'=1+2"));
    assert!(records[0][6].contains("\n第二行,\"quoted\""));
    let html = fs::read_to_string(f.0.join("result.html")).unwrap();
    assert!(html.contains("&lt;团队 &amp; 合成&gt;"));
    assert!(!html.contains("<团队"));
    let xss = f.0.join("xss.html");
    db.export_conversation("M:9007199254740993", &xss, ExportFormat::Html)
        .unwrap();
    let html = fs::read_to_string(xss).unwrap();
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(db
        .export_conversation("R:team", &f.0.join("input/new.json"), ExportFormat::Json)
        .is_err());
    assert!(db
        .export_conversation("missing", &f.0.join("missing.json"), ExportFormat::Json)
        .is_err());
    assert!(!f.0.join("missing.json").exists());
    assert!(!fs::read_dir(&f.0).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".wx-enterprise-export")));
    f.unchanged();
}

#[test]
fn optional_contact_databases_and_explicit_self_identity() {
    let f = Fixture::new();
    fs::remove_file(f.0.join("input/user.db")).unwrap();
    fs::remove_file(f.0.join("input/session.db")).unwrap();
    let db = OfflineStore::open(&f.0.join("input"), None).unwrap();
    assert!(db.contacts().unwrap().is_empty());
    let messages = db.messages(&MessageFilter::default()).unwrap();
    assert_eq!(messages.len(), 7);
    assert!(messages.iter().all(|m| !m.is_sent));
    assert_eq!(messages[0].sender, SELF_ID.to_string());
}

#[test]
fn missing_corrupt_sidecar_and_schema_failures_do_not_create_files() {
    let f = Fixture::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = f.0.join(format!("input/message.db{suffix}"));
        fs::write(&path, b"synthetic").unwrap();
        assert!(OfflineStore::open(&f.0.join("input"), None).is_err());
        fs::remove_file(path).unwrap();
    }
    fs::remove_file(f.0.join("input/message.db")).unwrap();
    assert!(OfflineStore::open(&f.0.join("input"), None).is_err());
    assert!(!f.0.join("input/message.db").exists());
    fs::write(f.0.join("input/message.db"), b"not SQLite").unwrap();
    assert!(OfflineStore::open(&f.0.join("input"), None).is_err());
    fs::remove_file(f.0.join("input/message.db")).unwrap();
    let db = Connection::open(f.0.join("input/message.db")).unwrap();
    db.execute_batch("CREATE TABLE message_table (conversation_id TEXT,send_time INTEGER)")
        .unwrap();
    drop(db);
    let db = f.store();
    assert!(db.messages(&MessageFilter::default()).is_err());
}
