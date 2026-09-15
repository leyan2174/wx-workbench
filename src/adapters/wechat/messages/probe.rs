//! Bounded session-cache diagnostic, separate from conversation inventory.
use anyhow::{ensure, Result};
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

pub const fn source_key() -> &'static str {
    "session/session.db"
}

pub struct Observation {
    pub rows_read: usize,
    pub latest_timestamp: Option<i64>,
}

pub fn observe(path: &Path, limit: usize) -> Result<Observation> {
    ensure!(
        (1..=10_000).contains(&limit),
        "延迟探测行数上限须在 1..10000 内"
    );
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(1))?;
    let mut statement = conn.prepare(
        "SELECT last_timestamp FROM SessionTable WHERE last_timestamp > 0 ORDER BY last_timestamp DESC LIMIT ?1",
    )?;
    let mut rows = statement.query([limit as i64])?;
    let mut observation = Observation {
        rows_read: 0,
        latest_timestamp: None,
    };
    while let Some(row) = rows.next()? {
        let timestamp: i64 = row.get(0)?;
        ensure!(timestamp > 0, "延迟探测查询返回无效时间戳");
        observation.rows_read += 1;
        observation.latest_timestamp = Some(
            observation
                .latest_timestamp
                .map_or(timestamp, |old| old.max(timestamp)),
        );
    }
    Ok(observation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_positive_timestamps_preserve_duplicates_and_source_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE SessionTable(last_timestamp); INSERT INTO SessionTable VALUES (0),(-1),(12),(12),(8),(NULL);").unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        let result = observe(&path, 2).unwrap();
        assert_eq!(result.rows_read, 2);
        assert_eq!(result.latest_timestamp, Some(12));
        assert_eq!(observe(&path, 10).unwrap().rows_read, 3);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(observe(&path, 0).is_err());
        assert!(observe(&path, 10_001).is_err());
    }

    #[test]
    fn unavailable_unknown_schema_and_bad_values_are_not_empty_success() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.db");
        assert!(observe(&path, 1).is_err());
        assert!(!path.exists());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE SessionTable(wrong_column INTEGER);")
            .unwrap();
        assert!(observe(&path, 1).is_err());
        conn.execute_batch("DROP TABLE SessionTable; CREATE TABLE SessionTable(last_timestamp);")
            .unwrap();
        let empty = observe(&path, 1).unwrap();
        assert_eq!(empty.rows_read, 0);
        assert_eq!(empty.latest_timestamp, None);
        conn.execute_batch("INSERT INTO SessionTable VALUES ('invalid');")
            .unwrap();
        assert!(observe(&path, 1).is_err());
    }
}
