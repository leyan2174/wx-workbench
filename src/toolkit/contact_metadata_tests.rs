use super::*;

#[test]
fn legacy_ast_golden() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/contact-metadata-golden.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic-contact.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(case["sql"].as_str().unwrap()).unwrap();
        conn.close().unwrap();
        let before = std::fs::read(&path).unwrap();
        let actual = contact_metadata_for_export(
            &path,
            case["username"].as_str().unwrap(),
            case["is_group"].as_bool().unwrap(),
        );
        assert_eq!(
            Value::Object(actual.fields),
            case["expected"],
            "case {}",
            case["name"]
        );
        assert_eq!(
            before,
            std::fs::read(&path).unwrap(),
            "read-only case {}",
            case["name"]
        );
    }
}

#[test]
fn absent_path_is_never_created_and_group_never_opens_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing.db");
    let group = contact_metadata_for_export(&path, "u", true);
    assert!(group.fields.is_empty());
    assert!(group.diagnostics.is_empty());
    let direct = contact_metadata_for_export(&path, "u", false);
    assert_eq!(direct.fields.len(), 4);
    assert_eq!(direct.diagnostics.len(), 1);
    assert!(!path.exists());
}

#[test]
fn schema_fallback_has_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE contact(username, nick_name, remark)")
        .unwrap();
    let actual = contact_metadata_for_export(&path, "u", false);
    assert!(actual
        .diagnostics
        .iter()
        .any(|s| s.contains("description missing")));
    assert!(actual
        .diagnostics
        .iter()
        .any(|s| s.contains("local_type missing")));
    assert!(actual
        .diagnostics
        .iter()
        .any(|s| s.contains("tags fallback")));
}

#[test]
fn corrupt_database_returns_empty_fields_and_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic-corrupt.db");
    std::fs::write(&path, b"not a SQLite database").unwrap();
    let actual = contact_metadata_for_export(&path, "u", false);
    assert_eq!(actual.fields["contact_nick_name"], "");
    assert_eq!(actual.fields["contact_tags"], Value::Array(vec![]));
    assert!(!actual.diagnostics.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"not a SQLite database");
}
