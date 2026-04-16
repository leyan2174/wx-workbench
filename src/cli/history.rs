use anyhow::Result;
use crate::ipc::Request;
use super::super::cli::transport;

pub fn cmd_history(
    chat: String,
    limit: usize,
    offset: usize,
    since: Option<String>,
    until: Option<String>,
    json: bool,
) -> Result<()> {
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(|s| parse_time_end(s)).transpose()?;

    let req = Request::History {
        chat,
        limit,
        offset,
        since: since_ts,
        until: until_ts,
    };

    let resp = transport::send(req)?;

    if json {
        let msgs = resp.data.get("messages").cloned().unwrap_or(serde_json::Value::Array(vec![]));
        println!("{}", serde_json::to_string_pretty(&msgs)?);
        return Ok(());
    }

    let chat_name = resp.data["chat"].as_str().unwrap_or("");
    let is_group = resp.data["is_group"].as_bool().unwrap_or(false);
    let count = resp.data["count"].as_i64().unwrap_or(0);
    let group_str = if is_group { " [群]" } else { "" };
    println!("=== {}{}  ({} 条) ===\n", chat_name, group_str, count);

    if let Some(msgs) = resp.data["messages"].as_array() {
        for m in msgs {
            let time = m["time"].as_str().unwrap_or("");
            let sender = m["sender"].as_str().unwrap_or("");
            let content = m["content"].as_str().unwrap_or("");

            let sender_str = if !sender.is_empty() {
                format!("\x1b[33m{}\x1b[0m: ", sender)
            } else {
                String::new()
            };

            println!("\x1b[90m[{}]\x1b[0m {}{}", time, sender_str, content);
        }
    }

    Ok(())
}

pub fn parse_time(s: &str) -> Result<i64> {
    for fmt in &["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(dt.and_utc().timestamp());
        }
        // 尝试仅日期格式
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            let dt = d.and_hms_opt(0, 0, 0).unwrap();
            return Ok(dt.and_utc().timestamp());
        }
    }
    anyhow::bail!("无法解析时间 '{}'，支持 YYYY-MM-DD / YYYY-MM-DD HH:MM / YYYY-MM-DD HH:MM:SS", s)
}

pub fn parse_time_end(s: &str) -> Result<i64> {
    // 对于仅日期格式，结束时间为当天 23:59:59
    if s.len() == 10 {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let dt = d.and_hms_opt(23, 59, 59).unwrap();
            return Ok(dt.and_utc().timestamp());
        }
    }
    parse_time(s)
}
