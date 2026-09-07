use super::*;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{fs, path::PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    db: PathBuf,
    attach: PathBuf,
    case: Value,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let oracle: Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()
            .clone();
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("resource.db");
        let attach = root.path().join("msg/attach");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ChatName2Id(user_name TEXT);
            INSERT INTO ChatName2Id VALUES('wxid_fixture'),('wxid_other');
            CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,
            message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);",
        )
        .unwrap();
        for row in case["resources"].as_array().unwrap() {
            let hex = row["packed_hex"].as_str().unwrap();
            let packed: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            conn.execute(
                "INSERT INTO MessageResourceInfo VALUES(?1,?2,?3,?4,?5)",
                params![
                    if row["username"] == "wxid_fixture" {
                        1
                    } else {
                        2
                    },
                    row["local_id"].as_i64().unwrap(),
                    row["local_type"].as_i64().unwrap(),
                    row["create_time"].as_i64().unwrap(),
                    packed,
                ],
            )
            .unwrap();
        }
        drop(conn);
        for file in case["files"].as_array().unwrap() {
            let path = attach
                .join(format!(
                    "{:x}",
                    md5::compute(file["username"].as_str().unwrap())
                ))
                .join(file["relative"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b'x'; file["size"].as_u64().unwrap() as usize]).unwrap();
        }
        Self {
            root,
            db,
            attach,
            case,
        }
    }

    fn messages(&self) -> Vec<(MessageIdentity, bool)> {
        self.case["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    MessageIdentity {
                        username: "wxid_fixture".into(),
                        source: format!("message/message_{}.db", row["shard"]),
                        local_id: row["local_id"].as_i64().unwrap(),
                        create_time: row["create_time"].as_i64().unwrap(),
                        local_type: row["local_type"].as_i64().unwrap(),
                    },
                    false,
                )
            })
            .collect()
    }

    fn read(&self) -> Result<Vec<ImageMetadata>> {
        read_page(
            if self.case["resource_available"] == false {
                None
            } else {
                Some(&self.db)
            },
            Some(&self.attach),
            &self.messages(),
        )
    }
}

fn contract(row: &ImageMetadata) {
    assert_eq!(row.resource_status == "found", row.md5.is_some());
    assert_eq!(row.size_status == "available", row.size.is_some());
    assert!(!(row.md5.is_none() && row.size.is_some()));
    assert_eq!(row.size_kind, "encrypted_dat_metadata");
    assert_eq!(row.binding, "exact_resource_standard_filename_metadata");
    let json = serde_json::to_value(row).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 6);
}

#[test]
fn exact_metadata_oracle_parity_and_explicit_differences() {
    for (name, resource, size) in [
        ("unique_resource_one_dat", "found", "available"),
        ("missing_resource_row", "missing", "not_requested"),
        ("missing_resource_database", "unavailable", "not_requested"),
        ("resource_without_md5", "md5_missing", "not_requested"),
        ("md5_but_not_downloaded", "found", "missing"),
        ("zero_byte_dat", "found", "available"),
        ("half_kib_rounds_to_even", "found", "available"),
        (
            "missing_exact_time_both_fallback",
            "missing",
            "not_requested",
        ),
        ("other_chat_same_id", "found", "available"),
        ("high_flag_resource", "missing", "not_requested"),
        ("high_flag_message_legacy_excluded", "found", "available"),
        (
            "ambiguous_exact_resource_legacy_first_row",
            "ambiguous",
            "not_requested",
        ),
        ("legacy_cross_month_lexical_thumbnail", "found", "ambiguous"),
        (
            "legacy_prefix_glob_accepts_noncanonical_name",
            "found",
            "missing",
        ),
        ("uppercase_marker_preserved", "found", "available"),
    ] {
        let f = Fixture::new(name);
        let before = fs::read(&f.db).unwrap();
        let rows = f.read().unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let row = &rows[0];
        contract(row);
        assert_eq!(row.resource_status, resource, "{name}");
        assert_eq!(row.size_status, size, "{name}");
        if resource == "found" && name != "high_flag_message_legacy_excluded" {
            assert_eq!(
                row.md5.as_deref(),
                f.case["legacy_rows"][0]["md5"]
                    .as_str()
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                "{name}"
            );
        }
        if size == "available" && name != "high_flag_message_legacy_excluded" {
            assert_eq!(
                row.size,
                f.case["legacy_rows"][0]["size"].as_u64(),
                "{name}"
            );
        }
        assert_eq!(fs::read(&f.db).unwrap(), before);
        assert!(!f.root.path().join("output").exists());
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!PathBuf::from(format!("{}{suffix}", f.db.display())).exists());
        }
    }
}

#[test]
fn missing_or_ambiguous_identity_never_requests_size() {
    let f = Fixture::new("unique_resource_one_dat");
    let mut messages = f.messages();
    messages[0].1 = true;
    let row = read_page(
        Some(Path::new("invalid")),
        Some(Path::new("invalid")),
        &messages,
    )
    .unwrap()
    .remove(0);
    contract(&row);
    assert_eq!(row.resource_status, "message_ambiguous");
    assert_eq!(row.size_status, "not_requested");
    let row = read_page(Some(&f.db), None, &f.messages())
        .unwrap()
        .remove(0);
    assert_eq!(row.resource_status, "found");
    assert_eq!(row.size_status, "not_requested");
    assert!(read_page(Some(Path::new("invalid")), None, &[])
        .unwrap()
        .is_empty());
}

#[test]
fn reused_id_uses_exact_time_and_raw_type() {
    let f = Fixture::new("reused_local_id_legacy_chooses_newest");
    let row = f.read().unwrap().remove(0);
    assert_ne!(row.md5.as_deref(), f.case["legacy_rows"][0]["md5"].as_str());
    let expected = &f.case["resources"][0];
    assert_eq!(
        expected["create_time"].as_i64(),
        Some(f.messages()[0].0.create_time)
    );
    let mut messages = f.messages();
    messages[0].0.local_type |= 1_i64 << 32;
    assert_eq!(
        read_page(Some(&f.db), Some(&f.attach), &messages).unwrap()[0].resource_status,
        "missing"
    );
}

#[test]
fn resource_schema_and_sidecars_are_errors_not_missing() {
    for sql in [
        "DROP TABLE MessageResourceInfo; CREATE VIEW MessageResourceInfo AS SELECT 1;",
        "ALTER TABLE MessageResourceInfo ADD COLUMN rowid INTEGER;",
        "UPDATE MessageResourceInfo SET packed_info='not a blob';",
        "UPDATE MessageResourceInfo SET packed_info=zeroblob(1048577);",
    ] {
        let f = Fixture::new("unique_resource_one_dat");
        Connection::open(&f.db).unwrap().execute_batch(sql).unwrap();
        assert!(f.read().is_err(), "{sql}");
    }
    let f = Fixture::new("unique_resource_one_dat");
    fs::write(PathBuf::from(format!("{}-wal", f.db.display())), b"sidecar").unwrap();
    assert!(f.read().is_err());
}

#[test]
fn duplicate_chat_mapping_is_ambiguous() {
    let f = Fixture::new("unique_resource_one_dat");
    Connection::open(&f.db)
        .unwrap()
        .execute("INSERT INTO ChatName2Id VALUES('wxid_fixture')", [])
        .unwrap();
    let row = f.read().unwrap().remove(0);
    contract(&row);
    assert_eq!(row.resource_status, "ambiguous");
}

#[test]
fn dat_is_metadata_only_and_not_subject_to_decoder_byte_limit() {
    let f = Fixture::new("unique_resource_one_dat");
    let file = &f.case["files"][0];
    let path = f
        .attach
        .join(format!("{:x}", md5::compute("wxid_fixture")))
        .join(file["relative"].as_str().unwrap());
    let size = super::super::native_image::MAX_DAT_BYTES + 1;
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size)
        .unwrap();
    let row = f.read().unwrap().remove(0);
    assert_eq!(row.size, Some(size));
    contract(&row);
    let second = path.with_file_name(format!("{}_t.dat", row.md5.unwrap()));
    fs::write(second, b"not an image").unwrap();
    let row = f.read().unwrap().remove(0);
    assert_eq!(row.size_status, "ambiguous");
    assert_eq!(row.size, None);
}
