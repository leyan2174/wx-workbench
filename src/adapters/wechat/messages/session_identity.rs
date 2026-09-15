//! Bounded, read-only session identities without message-content projections.
pub fn usernames(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare("SELECT username FROM SessionTable LIMIT 100001")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        rows.len() <= 100_000,
        crate::business::messages::Error::Limit
    );
    anyhow::ensure!(
        rows.iter().all(|name| !name.is_empty()),
        "session contains an empty username"
    );
    Ok(rows)
}
