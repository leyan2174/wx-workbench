use super::*;
use rusqlite::{params, Connection};
use std::{collections::HashMap, fs};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("合成 contact.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE contact_label(label_id_,label_name_,sort_order_); CREATE TABLE contact(username TEXT,extra_buffer BLOB,unknown_field TEXT);
            INSERT INTO contact_label VALUES(1,'朋友',20),(2,'Work',10),(3,'',30),(4,'WORK team',40),(5,'无人',50);").unwrap();
    for (user, ids) in [("u1", "１,2,2,3,unknown,999"), ("u2", "+1,٢"), ("u1", "1")] {
        let mut bytes = vec![
            8,
            1,
            17,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            26,
            1,
            b'x',
            37,
            0,
            0,
            0,
            0,
            242,
            1,
            ids.len() as u8,
        ];
        bytes.extend_from_slice(ids.as_bytes());
        conn.execute(
            "INSERT INTO contact VALUES(?1,?2,'ignored')",
            params![user, bytes],
        )
        .unwrap();
    }
    (dir, path)
}

#[test]
fn unicode_unknown_fields_duplicates_empty_labels_and_read_only() {
    let (_dir, path) = fixture();
    let before = fs::read(&path).unwrap();
    let tags =
        contact_tags_from_path(&path, &HashMap::from([("u1".into(), "张三".into())])).unwrap();
    assert_eq!(tags.total_tags, 5);
    assert_eq!(tags.total_associations, 7);
    assert_eq!(
        tags.tags
            .iter()
            .map(|t| (t.name.as_str(), t.member_count))
            .collect::<Vec<_>>(),
        vec![
            ("Work", 3),
            ("朋友", 3),
            ("", 1),
            ("WORK team", 0),
            ("无人", 0)
        ]
    );
    assert_eq!(tags.tags[0].members[0].display_name, "张三");
    assert_eq!(tags.tags[0].members[2].display_name, "u2");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn extracted_python_oracle_matches_complete_result() {
    let (_dir, path) = fixture();
    let names = HashMap::from([("u1".into(), "张三".into())]);
    let actual = contact_tags_from_path(&path, &names).unwrap();
    let output = std::process::Command::new(
        std::env::var_os("CONTACTS_ORACLE_PYTHON").unwrap_or_else(|| "python".into()),
    )
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mcp-contacts/oracle.py"
    ))
    .arg(&path)
    .env("PYTHONUTF8", "1")
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
}

#[test]
fn exact_precedes_contains_and_ambiguity_is_not_first_match() {
    let (_dir, path) = fixture();
    let mut tags = contact_tags_from_path(&path, &HashMap::new()).unwrap();
    assert_eq!(select_tag(&tags, " wOrK ").unwrap().name, "Work");
    assert_eq!(select_tag(&tags, "友").unwrap().name, "朋友");
    assert_eq!(select_tag(&tags, "").unwrap().name, "");
    assert!(select_tag(&tags, "wor")
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    assert!(select_tag(&tags, "missing").is_err());
    tags.tags.push(tags.tags[0].clone());
    assert!(select_tag(&tags, "work")
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
}

#[test]
fn repeated_definitions_numeric_types_and_first_field_match_legacy() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("INSERT INTO contact_label VALUES(1.0,'新名字',60),('2','文本ID',70);")
        .unwrap();
    conn.execute(
        "INSERT INTO contact VALUES('u3',?1,NULL)",
        [vec![242u8, 1, 50, b'1']],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO contact VALUES('u4',?1,NULL)",
        [vec![242u8, 1, 1, b'3', 242, 1, 1, b'1']],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO contact VALUES('u5',?1,NULL)",
        [vec![242u8, 1, 1, 255]],
    )
    .unwrap();
    let tags = contact_tags_from_path(&path, &HashMap::new()).unwrap();
    assert_eq!(select_tag(&tags, "新名字").unwrap().member_count, 4);
    assert_eq!(select_tag(&tags, "文本ID").unwrap().member_count, 0);
    assert_eq!(select_tag(&tags, "").unwrap().member_count, 2);
}

#[test]
fn unicode_names_and_python_integer_separators() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("INSERT INTO contact_label VALUES(12,'ÄPFEL',80)")
        .unwrap();
    let ids = " +１_２ ,12_,_12,1__2,١٢";
    let mut bytes = vec![242u8, 1, ids.len() as u8];
    bytes.extend_from_slice(ids.as_bytes());
    conn.execute("INSERT INTO contact VALUES('u6',?1,NULL)", [bytes])
        .unwrap();
    let tags = contact_tags_from_path(&path, &HashMap::new()).unwrap();
    assert_eq!(select_tag(&tags, " äpfel ").unwrap().member_count, 2);
    assert_eq!(select_tag(&tags, "äpf").unwrap().name, "ÄPFEL");
}

#[test]
fn malformed_association_does_not_return_prior_partial_results() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("INSERT INTO contact VALUES('bad','not a blob',NULL)")
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(contact_tags_from_path(&path, &HashMap::new()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn missing_corrupt_and_missing_schema_are_not_successful_empty_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.db");
    assert!(contact_tags_from_path(&path, &HashMap::new()).is_err());
    assert!(!path.exists());
    fs::write(&path, "not sqlite").unwrap();
    assert!(contact_tags_from_path(&path, &HashMap::new()).is_err());
    let empty = dir.path().join("empty.db");
    let conn = Connection::open(&empty).unwrap();
    assert!(contact_tags_from_path(&empty, &HashMap::new()).is_err());
    conn.execute_batch("CREATE TABLE contact_label(label_id_,label_name_,sort_order_)")
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&empty, &HashMap::new())
            .unwrap()
            .total_tags,
        0
    );
}

#[tokio::test]
async fn account_cache_boundary_has_no_fallback_to_other_account() {
    let (dir, path) = fixture();
    let cache = seeded_cache(dir.path(), &path, "contact\\contact.db", false).await;
    assert_eq!(
        q_tag_members(&cache, &HashMap::new(), "work")
            .await
            .unwrap()
            .member_count,
        3
    );
    let other = tempfile::tempdir().unwrap();
    let other_cache = DbCache::with_dirs(
        other.path().join("source"),
        other.path().join("cache"),
        other.path().join("mtimes.json"),
        HashMap::new(),
    )
    .await
    .unwrap();
    assert!(q_contact_tags(&other_cache, &HashMap::new()).await.is_err());
}

async fn seeded_cache(root: &Path, path: &Path, key: &str, invalid_key: bool) -> DbCache {
    let source = root.join("source");
    fs::create_dir_all(source.join("contact")).unwrap();
    let raw = source.join("contact/contact.db");
    fs::write(&raw, b"synthetic encrypted placeholder").unwrap();
    let modified = fs::metadata(&raw)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mtimes = root.join("mtimes.json");
    fs::write(
        &mtimes,
        serde_json::json!({key: {"db_mt":modified,"wal_mt":0,"path":path}}).to_string(),
    )
    .unwrap();
    let value = if invalid_key {
        "invalid synthetic key".into()
    } else {
        "00".repeat(32)
    };
    DbCache::with_dirs(
        source,
        root.join("cache"),
        mtimes,
        HashMap::from([(key.into(), value)]),
    )
    .await
    .unwrap()
}

fn single_label_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, path) = fixture();
    Connection::open(&path).unwrap().execute_batch(
            "DELETE FROM contact; DELETE FROM contact_label; INSERT INTO contact_label VALUES(1,'',0)"
        ).unwrap();
    (dir, path)
}

fn associations(count: usize) -> Vec<u8> {
    let ids = vec!["1"; count].join(",");
    let mut data = vec![242, 1];
    let mut length = ids.len();
    loop {
        let mut byte = (length & 127) as u8;
        length >>= 7;
        if length != 0 {
            byte |= 128;
        }
        data.push(byte);
        if length == 0 {
            break;
        }
    }
    data.extend_from_slice(ids.as_bytes());
    data
}

#[test]
fn label_definition_limit_is_inclusive_and_counts_duplicate_ids() {
    let (_dir, path) = single_label_fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute("WITH RECURSIVE n(x) AS (SELECT 2 UNION ALL SELECT x+1 FROM n WHERE x < ?1) INSERT INTO contact_label SELECT x,'',x FROM n", [MAX_LABELS]).unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap()
            .total_tags,
        MAX_LABELS
    );
    // 多出的重复定义也必须失败，不能按去重后的标签数绕过限额。
    conn.execute_batch("INSERT INTO contact_label VALUES(1,'',0)")
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap_err()
            .to_string(),
        "contact label limit exceeded"
    );
}

#[test]
fn association_limit_is_inclusive_across_contact_rows() {
    let (_dir, path) = single_label_fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO contact VALUES('u',?1,NULL)",
        [associations(MAX_ASSOCIATIONS)],
    )
    .unwrap();
    let tags = contact_tags_from_path(&path, &HashMap::new()).unwrap();
    assert_eq!(tags.total_associations, MAX_ASSOCIATIONS);
    conn.execute("INSERT INTO contact VALUES('u',?1,NULL)", [associations(1)])
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap_err()
            .to_string(),
        "contact association limit exceeded"
    );
}

#[test]
fn buffer_limit_is_inclusive_even_when_first_field_is_small() {
    let (_dir, path) = single_label_fixture();
    let conn = Connection::open(&path).unwrap();
    let mut data = associations(1);
    data.resize(MAX_BUFFER_BYTES, 0);
    conn.execute("INSERT INTO contact VALUES('u',?1,NULL)", [&data])
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap()
            .total_associations,
        1
    );
    data.push(0);
    conn.execute("INSERT INTO contact VALUES('u',?1,NULL)", [&data])
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap_err()
            .to_string(),
        "contact buffer byte limit exceeded"
    );
}

#[test]
fn total_text_limit_is_inclusive_before_member_clones() {
    let (_dir, path) = single_label_fixture();
    let conn = Connection::open(&path).unwrap();
    let names = HashMap::from([("u".into(), "x".repeat(MAX_TEXT_BYTES - 1))]);
    let count = MAX_RESULT_TEXT_BYTES / MAX_TEXT_BYTES;
    conn.execute(
        "INSERT INTO contact VALUES('u',?1,NULL)",
        [associations(count)],
    )
    .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &names)
            .unwrap()
            .total_associations,
        count
    );
    conn.execute("INSERT INTO contact VALUES('u',?1,NULL)", [associations(1)])
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &names)
            .unwrap_err()
            .to_string(),
        "contact result text byte limit exceeded"
    );
}

#[tokio::test]
async fn query_byte_limit_is_inclusive_and_precedes_real_cache_access() {
    let (dir, path) = single_label_fixture();
    let conn = Connection::open(&path).unwrap();
    let query = "é".repeat(MAX_TEXT_BYTES / 2);
    conn.execute("UPDATE contact_label SET label_name_=?1", [&query])
        .unwrap();
    let tags = contact_tags_from_path(&path, &HashMap::new()).unwrap();
    assert_eq!(select_tag(&tags, &query).unwrap().name, query);
    let cache = seeded_cache(dir.path(), &path, "contact/contact.db", true).await;
    // 若越过前置验证，真实 get 会因合成坏密钥失败，而不是返回查询长度错误。
    assert!(q_contact_tags(&cache, &HashMap::new()).await.is_err());
    let oversized = query + "x";
    assert_eq!(
        q_tag_members(&cache, &HashMap::new(), &oversized)
            .await
            .unwrap_err()
            .to_string(),
        "tag query byte limit exceeded"
    );
    assert_eq!(
        select_tag(&tags, &oversized).unwrap_err().to_string(),
        "tag query byte limit exceeded"
    );
    conn.execute("UPDATE contact_label SET label_name_=?1", [&oversized])
        .unwrap();
    assert_eq!(
        contact_tags_from_path(&path, &HashMap::new())
            .unwrap_err()
            .to_string(),
        "contact text byte limit exceeded"
    );
}
