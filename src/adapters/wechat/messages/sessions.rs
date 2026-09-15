//! SessionTable compatibility profile; no message inventory decisions are made here.
pub use super::session_identity::usernames;

pub fn last_timestamp(path: &std::path::Path, username: &str) -> anyhow::Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn
        .query_row(
            "SELECT last_timestamp FROM SessionTable WHERE username = ?",
            [username],
            |row| row.get(0),
        )
        .optional()?)
}
use super::read::{decode_content, StoredContent, MAX_DECODED_BYTES, MAX_STORED_BYTES};
use super::{legacy, semantic_kind};
use crate::business::messages as domain;
use crate::business::sessions::Session;
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::{collections::HashMap, path::Path};

#[derive(Clone)]
pub struct Record {
    pub session: Session,
    pub type_label: String,
}

fn optional_text(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<String>> {
    match row.get_ref(index)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) => {
            ensure!(bytes.len() <= 4096, domain::Error::Limit);
            Ok(Some(std::str::from_utf8(bytes)?.to_owned()).filter(|s| !s.is_empty()))
        }
        _ => Err(domain::Error::InvalidData.into()),
    }
}
pub fn read(path: &Path, verified: &HashMap<String, i64>) -> Result<Vec<Record>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let conn = conn.unchecked_transaction()?;
    let mut statement = conn.prepare("SELECT username,unread_count,summary,last_timestamp,last_msg_type,last_msg_sender,last_sender_display_name FROM SessionTable LIMIT 100001")?;
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    let mut total = 0usize;
    while let Some(row) = rows.next()? {
        ensure!(result.len() < 100_000, domain::Error::Limit);
        let username = optional_text(row, 0)?.ok_or(domain::Error::InvalidData)?;
        let summary = match row.get_ref(2)? {
            ValueRef::Null => String::new(),
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => decode_summary(bytes)?,
            _ => anyhow::bail!(domain::Error::InvalidData),
        };
        total = total
            .checked_add(summary.len())
            .context("session budget overflow")?;
        ensure!(total <= 64 * 1_048_576, domain::Error::Limit);
        let code = row.get::<_, Option<i64>>(4)?.unwrap_or(0);
        result.push(Record {
            type_label: legacy::fmt_type(code),
            session: Session {
                kind: super::super::contacts::kind(
                    &username,
                    verified.get(&username).copied().unwrap_or(0) != 0,
                ),
                username,
                unread: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                timestamp: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                last_kind: semantic_kind(code),
                sender: optional_text(row, 5)?,
                sender_display_hint: optional_text(row, 6)?,
                summary: crate::message::split_group_content(&summary).1.to_owned(),
            },
        });
    }
    Ok(result)
}

pub fn decode_summary(bytes: &[u8]) -> Result<String> {
    ensure!(bytes.len() <= MAX_STORED_BYTES, domain::Error::Limit);
    let content = StoredContent::Blob(bytes.to_vec());
    let decoded = decode_content(
        &content,
        bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]).then_some(4),
        MAX_DECODED_BYTES,
    )?;
    Ok(String::from_utf8(decoded)?)
}

/// Legacy timestamp subscription input, explicitly not the message directory.
pub fn timestamps(path: &Path) -> Result<Vec<(String, i64)>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare(
        "SELECT username,last_timestamp FROM SessionTable WHERE last_timestamp>0 LIMIT 100001",
    )?;
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(rows.len() <= 100_000, domain::Error::Limit);
    Ok(rows)
}

/// Narrow publisher/unread directory profile: no summary, sender, or timestamp columns required.
pub fn unread_publishers(conn: &Connection) -> Result<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT username FROM SessionTable WHERE unread_count > 0 LIMIT 100001")?;
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        count += 1;
        ensure!(count <= 100_000, domain::Error::Limit);
        result.push(optional_text(row, 0)?.ok_or(domain::Error::InvalidData)?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unread_directory_needs_only_the_two_actual_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE SessionTable(username TEXT,unread_count INTEGER); INSERT INTO SessionTable VALUES('a',1),('b',0),('a',2)").unwrap();
        assert_eq!(unread_publishers(&conn).unwrap(), ["a", "a"]);
        conn.execute("INSERT INTO SessionTable VALUES(NULL,1)", [])
            .unwrap();
        assert!(unread_publishers(&conn).is_err());
    }
    #[test]
    fn malformed_unread_identity_is_not_an_empty_success() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE SessionTable(username BLOB,unread_count INTEGER); INSERT INTO SessionTable VALUES(X'ff',1)").unwrap();
        assert!(unread_publishers(&conn).is_err());
    }
}
