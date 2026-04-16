use anyhow::Result;
use crate::ipc::Request;
use super::super::cli::transport;

pub fn cmd_sessions(limit: usize, json: bool) -> Result<()> {
    let resp = transport::send(Request::Sessions { limit })?;
    let data = resp.data.get("sessions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    for s in &data {
        let time = s["time"].as_str().unwrap_or("");
        let chat = s["chat"].as_str().unwrap_or("");
        let is_group = s["is_group"].as_bool().unwrap_or(false);
        let unread = s["unread"].as_i64().unwrap_or(0);
        let msg_type = s["last_msg_type"].as_str().unwrap_or("");
        let sender = s["last_sender"].as_str().unwrap_or("");
        let summary = s["summary"].as_str().unwrap_or("");

        let unread_str = if unread > 0 {
            format!(" \x1b[31m({}未读)\x1b[0m", unread)
        } else {
            String::new()
        };
        let group_str = if is_group { " [群]" } else { "" };
        let sender_str = if !sender.is_empty() {
            format!("{}: ", sender)
        } else {
            String::new()
        };

        println!("\x1b[90m[{}]\x1b[0m \x1b[1m{}\x1b[0m{}{}", time, chat, group_str, unread_str);
        println!("  {}: {}{}", msg_type, sender_str, summary);
        println!();
    }

    Ok(())
}
