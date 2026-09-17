//! Exact alias projection for raw directory exports, not contact identity resolution.
use anyhow::{Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::path::Path;

pub(crate) fn read(path: &Path, username: &str) -> Result<Option<String>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let snapshot = conn.unchecked_transaction()?;
    let columns = snapshot
        .prepare("PRAGMA table_info(contact)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case("alias"))
    {
        return Ok(None);
    }
    let filter = if columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case("local_type"))
    {
        " AND local_type != 3"
    } else {
        ""
    };
    let mut statement = snapshot.prepare(&format!(
        "SELECT alias FROM contact WHERE username COLLATE BINARY = ?1{filter} LIMIT 1"
    ))?;
    let mut rows = statement.query([username])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    match row.get_ref(0)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) => Ok(Some(
            std::str::from_utf8(bytes)
                .context("contact.alias 不是有效 UTF-8")?
                .into(),
        )),
        _ => anyhow::bail!("contact.alias 必须为 SQLite TEXT 或 NULL"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_username_filter_and_first_row_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contacts.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE contact(username TEXT,alias,local_type INTEGER);
             INSERT INTO contact VALUES('Alice','other',1),('alice','excluded',3),
               ('alice','first',1),('alice','second',1),('empty',NULL,1);",
            )
            .unwrap();
        assert_eq!(read(&path, "alice").unwrap().as_deref(), Some("first"));
        assert_eq!(read(&path, "Alice").unwrap().as_deref(), Some("other"));
        assert_eq!(read(&path, "ALICE").unwrap(), None);
        assert_eq!(read(&path, "empty").unwrap(), None);
    }

    #[test]
    fn optional_columns_and_invalid_values_keep_distinct_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contacts.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE contact(username TEXT); INSERT INTO contact VALUES('alice')",
        )
        .unwrap();
        assert_eq!(read(&path, "alice").unwrap(), None);
        conn.execute_batch(
            "ALTER TABLE contact ADD COLUMN alias; UPDATE contact SET alias='legacy'",
        )
        .unwrap();
        assert_eq!(read(&path, "alice").unwrap().as_deref(), Some("legacy"));
        for value in ["42", "x'6162'", "CAST(x'FF' AS TEXT)"] {
            conn.execute_batch(&format!("UPDATE contact SET alias={value}"))
                .unwrap();
            assert!(read(&path, "alice").is_err());
        }
    }
}
