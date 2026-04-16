use anyhow::Result;
use crate::ipc::Request;
use super::super::cli::transport;
use super::history::{parse_time, parse_time_end};

pub fn cmd_search(
    keyword: String,
    chats: Vec<String>,
    limit: usize,
    since: Option<String>,
    until: Option<String>,
    json: bool,
) -> Result<()> {
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;

    let chats_opt = if chats.is_empty() { None } else { Some(chats) };

    let req = Request::Search {
        keyword: keyword.clone(),
        chats: chats_opt,
        limit,
        since: since_ts,
        until: until_ts,
    };

    let resp = transport::send(req)?;
    let results = resp.data.get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let count = resp.data["count"].as_i64().unwrap_or(results.len() as i64);

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    println!("搜索 \"{}\"，找到 {} 条:\n", keyword, count);
    for r in &results {
        let time = r["time"].as_str().unwrap_or("");
        let chat = r["chat"].as_str().unwrap_or("");
        let sender = r["sender"].as_str().unwrap_or("");
        let content = r["content"].as_str().unwrap_or("");

        let chat_str = if !chat.is_empty() {
            format!("\x1b[36m[{}]\x1b[0m ", chat)
        } else {
            String::new()
        };
        let sender_str = if !sender.is_empty() {
            format!("\x1b[33m{}\x1b[0m: ", sender)
        } else {
            String::new()
        };

        println!("\x1b[90m[{}]\x1b[0m {}{}{}", time, chat_str, sender_str, content);
    }

    Ok(())
}
