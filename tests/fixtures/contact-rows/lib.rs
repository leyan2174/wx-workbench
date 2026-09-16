#[path = "../../../src/business/contacts.rs"]
#[allow(unfulfilled_lint_expectations)]
pub mod contact_business;
pub mod business {
    pub use crate::contact_business as contacts;
}

#[path = "../../../src/adapters/wechat/contacts/mod.rs"]
#[allow(dead_code, unused_imports)]
pub mod contact_adapter;
pub mod adapters {
    pub mod wechat {
        pub use crate::contact_adapter as contacts;
    }
}

#[path = "../../../src/daemon/query/contact_rows.rs"]
#[cfg(test)]
pub mod contact_rows;

#[cfg(test)]
mod tests {
    use super::{contact_adapter::SqliteContacts, contact_business as domain, contact_rows};
    use domain::ContactSource;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::{fs, path::Path};

    fn database(path: &Path) {
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,
                local_type INTEGER,alias TEXT,description TEXT,phone TEXT,mobile TEXT);
             INSERT INTO contact VALUES
                ('b','HiddenNick','VisibleRemark',0,1,'alias-only','memo-only','first','second'),
                ('a','Alpha','',0,1,NULL,NULL,NULL,NULL),
                ('room@chatroom','Group','',0,1,NULL,NULL,NULL,NULL),
                ('gh_public','Public','',8,3,NULL,NULL,NULL,NULL),
                ('brandsessionholder','System','',0,1,NULL,NULL,NULL,NULL);",
            )
            .unwrap();
    }

    fn contacts(path: &Path, query: Option<&str>, limit: usize) -> anyhow::Result<Value> {
        contact_rows::project_page(domain::list(
            &SqliteContacts::new(path.into()),
            domain::ContactQuery {
                text: query,
                offset: 0,
                limit,
            },
        )?)
    }

    #[test]
    fn formal_people_contract_search_order_total_and_zero_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("contacts.db");
        database(&path);
        let before = fs::read(&path).unwrap();
        let all = json!({"contacts":[{"username":"a","display":"Alpha"},
            {"username":"b","display":"VisibleRemark"}],"total":2});
        assert_eq!(contacts(&path, None, 50).unwrap(), all);
        assert_eq!(contacts(&path, Some(""), 50).unwrap(), all);
        assert_eq!(
            contacts(&path, None, 1).unwrap(),
            json!({"contacts":[{"username":"a","display":"Alpha"}],"total":2})
        );
        assert_eq!(
            contacts(&path, None, 0).unwrap(),
            json!({"contacts":[],"total":2})
        );
        assert_eq!(
            contacts(&path, Some("visible"), 10).unwrap(),
            json!({"contacts":[{"username":"b","display":"VisibleRemark"}],"total":1})
        );
        for query in [
            "HiddenNick",
            "alias-only",
            "memo-only",
            "first",
            "%' OR 1=1 --",
        ] {
            assert_eq!(
                contacts(&path, Some(query), 50).unwrap(),
                json!({"contacts":[],"total":0})
            );
        }
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn metadata_is_preserved_in_business_objects_not_leaked_into_contact_wire() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("contacts.db");
        database(&path);
        let entries = SqliteContacts::new(path.clone()).contacts().unwrap();
        let person = entries
            .contacts
            .iter()
            .find(|entry| entry.id.0 == "b")
            .unwrap();
        assert_eq!(person.phone.as_deref(), Some("first"));
        assert_eq!(person.alias.as_deref(), Some("alias-only"));
        assert_eq!(person.description.as_deref(), Some("memo-only"));
        assert!(entries
            .contacts
            .iter()
            .any(|entry| entry.id.0 == "room@chatroom"));
        for row in contacts(&path, None, 50).unwrap()["contacts"]
            .as_array()
            .unwrap()
        {
            assert_eq!(row.as_object().unwrap().len(), 2);
        }
    }

    #[test]
    fn invalid_sources_fields_queries_and_duplicate_identities_fail_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("contacts.db");
        assert!(contacts(&path, None, 50).is_err());
        assert!(!path.exists());
        assert!(contacts(&path, Some(&"q".repeat(4097)), 50).is_err());
        assert!(!path.exists());
        database(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("INSERT INTO contact(username,nick_name,remark,verify_flag) VALUES('a','Duplicate','',0)").unwrap();
        assert!(contacts(&path, None, 50).is_err());
        conn.execute_batch("DELETE FROM contact WHERE nick_name='Duplicate'; UPDATE contact SET nick_name=x'1234' WHERE username='a'").unwrap();
        assert!(contacts(&path, None, 50).is_err());
        conn.execute(
            "UPDATE contact SET nick_name=?1 WHERE username='a'",
            ["x".repeat(4097)],
        )
        .unwrap();
        assert!(contacts(&path, None, 50).is_err());
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert!(contacts(&path, None, 50).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn empty_directory_is_unavailable_but_unmatched_filter_is_empty() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("contacts.db");
        database(&path);
        assert_eq!(
            contacts(&path, Some("synthetic-unmatched-contact"), 50).unwrap(),
            json!({"contacts":[],"total":0})
        );
        Connection::open(&path)
            .unwrap()
            .execute("DELETE FROM contact", [])
            .unwrap();
        let before = fs::read(&path).unwrap();
        let error = contacts(&path, None, 50).unwrap_err();
        assert_eq!(error.downcast_ref(), Some(&domain::Error::Unavailable));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
