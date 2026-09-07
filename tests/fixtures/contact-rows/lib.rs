#[path = "../../../src/daemon/query/contact_rows.rs"]
pub mod contact_rows;

#[cfg(test)]
mod tests {
    use super::contact_rows::contacts_from_path;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::{fs, path::Path, process::Command};

    fn database(path: &Path, modern: bool) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT)")
            .unwrap();
        if modern {
            conn.execute_batch("ALTER TABLE contact ADD COLUMN local_type INTEGER DEFAULT 1;
                ALTER TABLE contact ADD COLUMN alias TEXT; ALTER TABLE contact ADD COLUMN description TEXT;
                ALTER TABLE contact ADD COLUMN phone TEXT; ALTER TABLE contact ADD COLUMN mobile TEXT").unwrap();
        }
        for (username, nick, remark) in [
            ("wxid_1", "HiddenNick", "VisibleRemark"),
            ("room@chatroom", "Group", ""),
            ("gh_public", "Public", ""),
            ("brandsessionholder", "System", ""),
            ("wxid_1", "Duplicate", ""),
            ("other", "", ""),
            ("wxid_İ", "ΟΣ", ""),
        ] {
            conn.execute(
                "INSERT INTO contact(username,nick_name,remark) VALUES(?1,?2,?3)",
                (username, nick, remark),
            )
            .unwrap();
        }
        if modern {
            conn.execute_batch("UPDATE contact SET alias='alias-only',description='memo-only',phone='',mobile='123456' WHERE username='other';
                INSERT INTO contact(username,nick_name,remark,local_type) VALUES('member','Member','',3),('null-type','Null','',NULL)").unwrap();
        }
    }

    #[test]
    fn real_sqlite_matches_legacy_ast_golden() {
        for modern in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("contact.db");
            database(&path, modern);
            let before = fs::read(&path).unwrap();
            for (query, limit) in [
                ("", 50),
                ("hiddenNICK", 1),
                ("VISIBLE", 50),
                ("wxid", 1),
                ("", 0),
                ("alias-only", 50),
                ("memo-only", 50),
                ("123456", 50),
                (" ", 50),
                ("ος", 50),
            ] {
                let output = Command::new("python")
                    .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle.py"))
                    .arg(&path)
                    .arg(query)
                    .arg(limit.to_string())
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let expected: Value = serde_json::from_slice(&output.stdout).unwrap();
                let mut actual = contacts_from_path(&path, Some(query), limit).unwrap();
                for contact in actual["contacts"].as_array_mut().unwrap() {
                    let expected_display = ["remark", "nick_name", "username"]
                        .iter()
                        .map(|k| contact[*k].as_str().unwrap())
                        .find(|s| !s.is_empty())
                        .unwrap();
                    assert_eq!(contact["display"], expected_display);
                    contact.as_object_mut().unwrap().remove("display");
                }
                assert_eq!(actual["contacts"], expected["contacts"], "{modern} {query}");
                assert_eq!(actual["total"], expected["total"]);
            }
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn invalid_sources_and_fields_fail_without_creating_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        assert!(contacts_from_path(&path, None, 50).is_err());
        assert!(!path.exists());
        assert!(contacts_from_path(&path, Some(&"q".repeat(4097)), 50)
            .unwrap_err()
            .to_string()
            .contains("query"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT)")
            .unwrap();
        assert!(contacts_from_path(&path, None, 50).is_err());
        conn.execute_batch("INSERT INTO contact VALUES('valid',X'1234','')")
            .unwrap();
        assert!(contacts_from_path(&path, None, 50).is_err());
        conn.execute("UPDATE contact SET nick_name=?1", ["x".repeat(4097)])
            .unwrap();
        assert!(contacts_from_path(&path, None, 50).is_err());
    }

    #[test]
    fn phone_priority_optional_nulls_and_literal_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contact.db");
        database(&path, true);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "UPDATE contact SET phone='first',mobile='second',remark=NULL WHERE username='other'",
        )
        .unwrap();
        let result = contacts_from_path(&path, Some("other"), 50).unwrap();
        assert_eq!(result["contacts"][0]["phone"], "first");
        assert_eq!(result["contacts"][0]["remark"], "");
        assert_eq!(result["contacts"][0]["display"], "other");
        assert_eq!(
            contacts_from_path(&path, Some("%' OR 1=1 --"), 50).unwrap(),
            json!({"contacts":[],"total":0})
        );
    }
}
