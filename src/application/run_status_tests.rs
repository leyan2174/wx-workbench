use super::*;
use serde_json::json;

#[test]
fn missing_config_and_directories_are_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let status = inspect(&dir.path().join("missing/config.json"), None).unwrap();
    assert!(!status.config_exists && !status.databases.exists && !status.exports.exists);
    assert!(status.key_files.is_empty());
    assert!(status.render().contains("wx database decrypt"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn counts_export_files_without_reading_keys_or_message_contents() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    fs::write(
        &config,
        br#"{"db_dir":"source","decrypted_dir":"plain","keys_file":"keys.json"}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("keys.json"),
        b"not-json-secret-key-material",
    )
    .unwrap();
    fs::write(dir.path().join("all_keys-old.json"), b"metadata-only").unwrap();
    fs::create_dir_all(dir.path().join("plain/nested")).unwrap();
    for (name, size) in [
        ("message_0.db", 1024),
        ("message_1.db", 2048),
        ("nested/contact.db", 3),
    ] {
        fs::write(dir.path().join("plain").join(name), vec![0; size]).unwrap();
    }
    let exports = dir.path().join("exported_chats");
    fs::create_dir(&exports).unwrap();
    fs::write(exports.join("plain.json"), b"0123456789").unwrap();
    fs::write(
        exports.join("one.json"),
        serde_json::to_vec(&json!({"messages":[
            {"type":"voice","annotation":"private-annotation","content":"private-message"},
            {"type":"voice"}, {"type":"text"}
        ]}))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        exports.join("many.json"),
        serde_json::to_vec(&json!({"messages":[{"type":"voice"}],"chats":[
            {"messages":[{"type":"voice"},{"type":"voice","annotation":{"text":"ok"}}]},
            {"messages":[{"type":"voice"},{"type":"voice"}]}
        ]}))
        .unwrap(),
    )
    .unwrap();
    fs::write(exports.join("unreadable.json"), b"{broken").unwrap();
    let status = inspect(&config, None).unwrap();
    assert_eq!(status.key_files.len(), 2);
    assert_eq!((status.databases.files, status.databases.bytes), (3, 3075));
    assert_eq!(
        (
            status.message_databases.files,
            status.message_databases.bytes
        ),
        (2, 3072)
    );
    assert_eq!(status.exports.files, 4);
    let bytes: u64 = fs::read_dir(&exports)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(status.exports.bytes, bytes);
    let output = format!(
        "{}{}",
        status.render(),
        serde_json::to_string(&status).unwrap()
    );
    for secret in [
        "not-json-secret-key-material",
        "private-annotation",
        "private-message",
    ] {
        assert!(!output.contains(secret));
    }
    assert!(!serde_json::to_value(status)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("progress"));
}
