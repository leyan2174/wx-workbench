//! Legacy MCP contacts use the current account cache, never global discovery.
use super::{contact_rows::contacts_from_path, DbCache};
use anyhow::{Context, Result};
use serde_json::Value;

pub async fn q_contacts_legacy(db: &DbCache, query: Option<&str>, limit: usize) -> Result<Value> {
    let path = match db.get("contact/contact.db").await? {
        Some(path) => path,
        None => db
            .get("contact\\contact.db")
            .await?
            .context("contact database unavailable")?,
    };
    let query = query.map(str::to_owned);
    tokio::task::spawn_blocking(move || contacts_from_path(&path, query.as_deref(), limit))
        .await
        .context("contact query task failed")?
}
