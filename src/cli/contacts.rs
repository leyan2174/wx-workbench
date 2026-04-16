use anyhow::Result;
use crate::ipc::Request;
use super::super::cli::transport;

pub fn cmd_contacts(query: Option<String>, limit: usize, json: bool) -> Result<()> {
    let req = Request::Contacts { query, limit };
    let resp = transport::send(req)?;

    let contacts = resp.data.get("contacts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let total = resp.data["total"].as_i64().unwrap_or(contacts.len() as i64);

    if json {
        println!("{}", serde_json::to_string_pretty(&contacts)?);
        return Ok(());
    }

    println!("共 {} 个联系人（显示 {} 个）:\n", total, contacts.len());
    for c in &contacts {
        let display = c["display"].as_str().unwrap_or("");
        let username = c["username"].as_str().unwrap_or("");
        println!("  {:<20} {}", display, username);
    }

    Ok(())
}
