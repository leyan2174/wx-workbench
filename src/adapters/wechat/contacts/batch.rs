//! Audio directory projection, not contact identity resolution.
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Default)]
pub struct Contact {
    pub alias: String,
    pub remark: String,
    pub nick_name: String,
}

pub fn read(path: &Path) -> Result<BTreeMap<String, Contact>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let mut query = connection.prepare("SELECT username, alias, remark, nick_name FROM contact")?;
    let mut result = BTreeMap::new();
    for row in query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Contact {
                alias: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                remark: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                nick_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            },
        ))
    })? {
        let (username, contact) = row?;
        result.insert(username, contact);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_nulls_and_last_duplicate_keep_directory_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("contact.db");
        assert!(read(&path).is_err());
        assert!(!path.exists());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE contact(username TEXT,alias TEXT,remark TEXT,nick_name TEXT); INSERT INTO contact VALUES ('alice','old','old','old'),('alice',NULL,'remark','nick');").unwrap();
        drop(conn);
        let projection = read(&path).unwrap();
        let contact = projection.get("alice").unwrap();
        assert_eq!(
            (&*contact.alias, &*contact.remark, &*contact.nick_name),
            ("", "remark", "nick")
        );
    }
}
