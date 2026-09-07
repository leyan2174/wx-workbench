use crate::{resolver, AttachmentId, AttachmentKind};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{fs, path::PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    resource: PathBuf,
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
        let resource = root.path().join("resource.db");
        let conn = Connection::open(&resource).unwrap();
        conn.execute_batch(
            "CREATE TABLE ChatName2Id(user_name TEXT);
            INSERT INTO ChatName2Id(rowid,user_name) VALUES(1,'wxid_fixture'),(2,'wxid_other');
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
            let hash = format!("{:x}", md5::compute(file["username"].as_str().unwrap()));
            let path = root
                .path()
                .join("msg/attach")
                .join(hash)
                .join(file["relative"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b'x'; file["size"].as_u64().unwrap() as usize]).unwrap();
        }
        Self {
            root,
            resource,
            case,
        }
    }

    fn id(&self) -> AttachmentId {
        let row = &self.case["messages"][0];
        AttachmentId {
            v: 1,
            chat: "wxid_fixture".into(),
            local_id: row["local_id"].as_i64().unwrap(),
            create_time: row["create_time"].as_i64().unwrap(),
            kind: AttachmentKind::Image,
            db: None,
        }
    }

    fn lookup(&self) -> Option<resolver::AttachmentMetadata> {
        let id = self.id();
        resolver::lookup_md5_blocking(&self.resource, &id.chat, id.local_id, id.create_time, 3)
            .unwrap()
    }

    fn resolve(&self) -> anyhow::Result<resolver::ResolvedAttachment> {
        resolver::resolve_blocking(
            &self.id(),
            &self.resource,
            &self.root.path().join("msg/attach"),
        )
    }
}

#[test]
fn unique_metadata_and_missing_local_file_have_distinct_sources() {
    let fixture = Fixture::new("unique_resource_one_dat");
    let before = fs::read(&fixture.resource).unwrap();
    let result = fixture.resolve().unwrap();
    assert_eq!(result.md5, fixture.case["legacy_rows"][0]["md5"]);
    assert_eq!(
        result.size,
        fixture.case["legacy_rows"][0]["size"].as_u64().unwrap()
    );
    assert_eq!(result.size, fs::metadata(&result.dat_path).unwrap().len());
    assert_eq!(fs::read(&fixture.resource).unwrap(), before);
    let fixture = Fixture::new("md5_but_not_downloaded");
    assert!(fixture.lookup().is_some());
    assert!(fixture.resolve().is_err());
    assert!(fixture.case["legacy_rows"][0].get("size").is_none());
    println!("PROVEN md5 comes from resource row, size from DAT metadata; missing DAT does not erase md5");
}

#[test]
fn old_latest_reuse_and_current_fallback_are_not_exact_identity_proof() {
    let fixture = Fixture::new("reused_local_id_legacy_chooses_newest");
    assert_ne!(
        fixture.lookup().unwrap().md5,
        fixture.case["legacy_rows"][0]["md5"]
    );
    let fixture = Fixture::new("missing_exact_time_both_fallback");
    assert_eq!(
        fixture.lookup().unwrap().md5,
        fixture.case["legacy_rows"][0]["md5"]
    );
    assert_ne!(
        fixture.case["messages"][0]["create_time"],
        fixture.case["resources"][0]["create_time"]
    );
    let fixture = Fixture::new("ambiguous_exact_resource_legacy_first_row");
    assert_ne!(
        fixture.lookup().unwrap().md5,
        fixture.case["legacy_rows"][0]["md5"]
    );
    println!("PROVEN exact-time improvement still has latest fallback; duplicate exact rows are not rejected");
}

#[test]
fn file_candidate_and_md5_case_rules_are_not_byte_for_byte_legacy_parity() {
    let fixture = Fixture::new("legacy_prefix_glob_accepts_noncanonical_name");
    assert_eq!(fixture.case["legacy_rows"][0]["size"], 777);
    assert!(fixture.resolve().is_err());
    let fixture = Fixture::new("uppercase_marker_preserved");
    let current = fixture.lookup().unwrap().md5;
    let old = fixture.case["legacy_rows"][0]["md5"].as_str().unwrap();
    assert_ne!(current, old);
    assert_eq!(current, old.to_ascii_lowercase());
    println!("PROVEN legacy broad prefix glob differs from canonical filename matching; Rust normalizes MD5 case");
}
