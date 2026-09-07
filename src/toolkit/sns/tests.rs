use super::decode::MAX_XML_CHARS;
use super::export::{
    load_comments, load_contacts, read_database, safe_dirname, write_export_with_media, ExportData,
    Timeline,
};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::FixedOffset;
use rusqlite::{params, types::Value as SqlValue, Connection};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

fn golden() -> Value {
    serde_json::from_str(include_str!("../../../tests/fixtures/sns/golden.json")).unwrap()
}
fn zone() -> TimeZone {
    TimeZone::Fixed(FixedOffset::east_opt(8 * 3600).unwrap())
}
fn payload(case: &Value) -> SqlValue {
    match case["encoding"].as_str().unwrap() {
        "blob_base64" => SqlValue::Blob(STANDARD.decode(case["input"].as_str().unwrap()).unwrap()),
        _ if case["input"].is_null() => SqlValue::Null,
        _ => SqlValue::Text(case["input"].as_str().unwrap().into()),
    }
}
fn content(value: &SqlValue) -> Content<'_> {
    match value {
        SqlValue::Text(s) => Content::Text(s),
        SqlValue::Blob(b) => Content::Blob(b),
        _ => Content::Null,
    }
}

#[test]
fn parser_matches_extracted_legacy_golden() {
    for case in golden()["cases"].as_array().unwrap() {
        let input = payload(case);
        let actual = parse_timeline(content(&input), zone());
        if case["expected"].is_null() {
            assert!(
                actual.is_err() || actual.unwrap().is_none(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                serde_json::to_value(actual.unwrap().unwrap()).unwrap(),
                case["expected"],
                "{}",
                case["name"]
            );
        }
    }
}

fn sql_value(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::String(s) => SqlValue::Text(s.clone()),
        _ => SqlValue::Integer(v.as_i64().unwrap()),
    }
}
fn databases(golden: &Value) -> (Connection, Connection) {
    let sns = Connection::open_in_memory().unwrap();
    sns.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content); CREATE TABLE SnsMessage_tmp3(feed_id INTEGER, create_time INTEGER, type INTEGER, from_username TEXT, from_nickname TEXT, to_username TEXT, to_nickname TEXT, content TEXT, del_status INTEGER);").unwrap();
    for row in golden["database"]["rows"].as_array().unwrap() {
        let case = golden["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == row["case"])
            .unwrap();
        sns.execute(
            "INSERT INTO SnsTimeLine VALUES (?,?,?)",
            params![
                row["tid"].as_i64().unwrap(),
                row["user_name"].as_str(),
                payload(case)
            ],
        )
        .unwrap();
    }
    for row in golden["database"]["comments"].as_array().unwrap() {
        sns.execute(
            "INSERT INTO SnsMessage_tmp3 VALUES (?,?,?,?,?,?,?,?,?)",
            rusqlite::params_from_iter(row.as_array().unwrap().iter().map(sql_value)),
        )
        .unwrap();
    }
    let contacts = Connection::open_in_memory().unwrap();
    contacts
        .execute_batch("CREATE TABLE contact(username TEXT, remark TEXT, nick_name TEXT);")
        .unwrap();
    for row in golden["database"]["contacts"].as_array().unwrap() {
        contacts
            .execute(
                "INSERT INTO contact VALUES (?,?,?)",
                rusqlite::params_from_iter(row.as_array().unwrap().iter().map(sql_value)),
            )
            .unwrap();
    }
    (sns, contacts)
}
fn options() -> ExportOptions {
    ExportOptions {
        timezone: zone(),
        export_time: Some(1700001000),
        ..Default::default()
    }
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "wx-sns-synthetic-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn database_and_files_match_legacy_golden() {
    let golden = golden();
    let (sns, contacts) = databases(&golden);
    let data = read_database(&sns, Some(&contacts), &options()).unwrap();
    assert_eq!(
        serde_json::to_value(&data.timelines).unwrap(),
        golden["database"]["timelines"]
    );
    assert_eq!((data.rows_seen, data.invalid, data.filtered), (6, 1, 0));
    let temp = Temp::new();
    let report = write_export_with_media(&data, &temp.0, zone(), None, None).unwrap();
    assert_eq!(
        (report.contacts, report.posts, report.files.len()),
        (3, 5, 11)
    );
    for (path, expected) in golden["database"]["files"].as_object().unwrap() {
        let actual: Value = serde_json::from_slice(&fs::read(temp.0.join(path)).unwrap()).unwrap();
        assert_eq!(&actual, expected, "{path}");
    }
    for timeline in &data.timelines {
        let dir = temp.0.join(&timeline.display_name).join("SNS");
        let summary: Value =
            serde_json::from_slice(&fs::read(dir.join("timeline.json")).unwrap()).unwrap();
        assert_eq!(summary, serde_json::to_value(timeline).unwrap());
        let html = fs::read_to_string(dir.join("timeline.html")).unwrap();
        assert!(!html.contains("<img") && !html.contains("<script"));
        assert!(html.contains("default-src 'none'"));
    }
    assert!(write_export_with_media(&data, &temp.0, zone(), None, None).is_err());
}

#[test]
fn filtering_and_missing_interactions_are_explicit() {
    let (sns, contacts) = databases(&golden());
    sns.execute_batch("DROP TABLE SnsMessage_tmp3;").unwrap();
    let mut options = options();
    options.contacts.insert("wxid_synthetic".into());
    let data = read_database(&sns, Some(&contacts), &options).unwrap();
    assert_eq!(
        (data.timelines.len(), data.filtered, data.invalid),
        (1, 2, 1)
    );
    assert!(data.warnings.iter().any(|s| s.contains("SnsMessage_tmp3")));
    assert!(data.timelines[0]
        .posts
        .iter()
        .all(|p| p.comments.as_ref().unwrap().is_empty()));
    sns.execute_batch("CREATE TABLE SnsMessage_tmp3(feed_id INTEGER);")
        .unwrap();
    assert!(load_comments(&sns, zone()).is_err());
}

#[test]
fn sqlite_content_storage_forms() {
    let golden = golden();
    let sns = Connection::open_in_memory().unwrap();
    sns.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content);")
        .unwrap();
    for (i, name) in [
        "rich",
        "raw_utf8",
        "zstd",
        "hex",
        "base64",
        "hex_zstd",
        "base64_zstd",
    ]
    .iter()
    .enumerate()
    {
        let case = golden["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == *name)
            .unwrap();
        sns.execute(
            "INSERT INTO SnsTimeLine VALUES (?, 'synthetic', ?)",
            params![i as i64, payload(case)],
        )
        .unwrap();
    }
    sns.execute_batch("INSERT INTO SnsTimeLine VALUES (100, 'synthetic', NULL);")
        .unwrap();
    let data = read_database(&sns, None, &options()).unwrap();
    assert_eq!(
        (data.rows_seen, data.invalid, data.timelines.len()),
        (7, 0, 1)
    );
    assert_eq!(data.timelines[0].posts.len(), 7);
    for post in &data.timelines[0].posts {
        assert_eq!(post.id, "18446744073709551610");
        assert_eq!(post.media[0]["url_token"], "synthetic-token");
    }
}

#[test]
fn timestamps_and_directory_safety() {
    assert_eq!(timestamp_filename(0, zone()).unwrap(), "00000000000000000");
    assert_eq!(
        timestamp_filename(1700000000, zone()).unwrap(),
        "20231115061320000"
    );
    assert!(timestamp_filename(i64::MAX, zone()).is_err());
    assert_eq!(safe_dirname("a:b/c*?"), "a_b_c__");
    for name in ["..", ".", "CON", "nul.txt", "LPT1", "COM¹", " ", "x."] {
        let safe = safe_dirname(name);
        assert_ne!(safe, name);
        assert!(!safe.ends_with('.'));
    }
}

#[test]
fn decoded_security_and_size_guards() {
    for xml in ["<!DoCtYpE x><x/>", "<!ENTITY x 'x'><x/>"] {
        let bytes = zstd::stream::encode_all(xml.as_bytes(), 1).unwrap();
        assert!(parse_timeline(Content::Blob(&bytes), zone()).is_err());
        assert!(parse_timeline(Content::Text(&STANDARD.encode(&bytes)), zone()).is_err());
    }
    let huge = "x".repeat(MAX_XML_CHARS + 1);
    assert!(decode_content(Content::Text(&huge)).is_err());
    let compressed =
        zstd::stream::encode_all("x".repeat(MAX_XML_CHARS * 4 + 1).as_bytes(), 1).unwrap();
    assert!(decode_content(Content::Blob(&compressed)).is_err());
    assert!(decode_content(Content::Blob(&[0x28, 0xb5, 0x2f, 0xfd, 0])).is_err());
}

#[test]
fn contact_collisions_are_not_merged() {
    let (sns, contacts) = databases(&golden());
    contacts
        .execute(
            "INSERT INTO contact VALUES (?,?,?)",
            params!["wxid_fallback", "合成:备注", ""],
        )
        .unwrap();
    let data = read_database(&sns, Some(&contacts), &options()).unwrap();
    let names: std::collections::BTreeSet<_> = data
        .timelines
        .iter()
        .map(|t| t.display_name.to_lowercase())
        .collect();
    assert_eq!(names.len(), data.timelines.len());
    let temp = Temp::new();
    assert_eq!(
        write_export_with_media(&data, &temp.0, zone(), None, None)
            .unwrap()
            .posts,
        5
    );
}

#[test]
fn read_only_path_entry_and_missing_path() {
    let temp = Temp::new();
    let path = temp.0.join("synthetic.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);")
            .unwrap();
        conn.execute("INSERT INTO SnsTimeLine VALUES (1, 'synthetic', ?)", ["<x><TimelineObject><contentDesc>&lt;script&gt;bad&lt;/script&gt;</contentDesc></TimelineObject></x>"]).unwrap();
    }
    let before = fs::read(&path).unwrap();
    let report =
        export_database_with_media(&path, None, &temp.0.join("out"), &options(), None, None)
            .unwrap();
    assert_eq!(report.posts, 1);
    assert_eq!(fs::read(&path).unwrap(), before);
    let missing = temp.0.join("missing.db");
    assert!(export_database_with_media(
        &missing,
        None,
        &temp.0.join("other"),
        &options(),
        None,
        None,
    )
    .is_err());
    assert!(!missing.exists());
    let html = fs::read_to_string(temp.0.join("out/synthetic/SNS/timeline.html")).unwrap();
    assert!(html.contains("&lt;script&gt;") && !html.contains("<script>"));
}

#[test]
fn rejects_manually_supplied_traversal() {
    let temp = Temp::new();
    let data = ExportData {
        timelines: vec![Timeline {
            user_name: "u".into(),
            display_name: "..".into(),
            export_time: "".into(),
            total_posts: 0,
            posts: vec![],
        }],
        ..Default::default()
    };
    assert!(write_export_with_media(&data, &temp.0, zone(), None, None).is_err());
    assert_eq!(
        serde_json::to_value(load_contacts(None).unwrap()).unwrap(),
        json!({})
    );
}
