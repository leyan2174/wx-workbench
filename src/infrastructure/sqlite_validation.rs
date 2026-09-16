use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

/// Validate a completed SQLite file without creating it or allowing writes.
pub fn validate_readonly_database(path: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open SQLite database read-only: {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    let status: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    ensure!(status == "ok", "SQLite database integrity check failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_database_and_rejects_non_database_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let valid = root.path().join("valid.db");
        let connection = Connection::open(&valid).unwrap();
        connection
            .execute_batch("CREATE TABLE synthetic(value TEXT); INSERT INTO synthetic VALUES('x');")
            .unwrap();
        drop(connection);
        let before = std::fs::read(&valid).unwrap();
        validate_readonly_database(&valid).unwrap();
        assert_eq!(std::fs::read(&valid).unwrap(), before);

        let invalid = root.path().join("invalid.db");
        std::fs::write(&invalid, b"not sqlite").unwrap();
        let before = std::fs::read(&invalid).unwrap();
        assert!(validate_readonly_database(&invalid).is_err());
        assert_eq!(std::fs::read(&invalid).unwrap(), before);
    }
}
