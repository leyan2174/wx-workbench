use super::output::{emit_warnings, print_response, OutputOpts};
use super::transport;
use crate::ipc::Request;
use anyhow::Result;

pub fn cmd_history(
    chat: String,
    limit: usize,
    offset: usize,
    since: Option<String>,
    until: Option<String>,
    msg_type: Option<String>,
    msg_types: Vec<String>,
    oldest_first: bool,
    opts: OutputOpts,
) -> Result<()> {
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;
    let type_val = msg_type.as_deref().and_then(parse_msg_type);
    anyhow::ensure!(
        msg_type.is_none() || msg_types.is_empty(),
        "不能同时指定 --type 和 --types"
    );
    let mut types = Vec::new();
    for name in msg_types {
        let value = parse_msg_type(&name).ok_or_else(|| anyhow::anyhow!("未知消息类型: {name}"))?;
        if !types.contains(&value) {
            types.push(value);
        }
    }
    let (with_meta, debug_source) = opts.request_flags();

    let req = Request::History {
        chat,
        limit,
        offset,
        since: since_ts,
        until: until_ts,
        msg_type: type_val,
        msg_types: (!types.is_empty()).then_some(types),
        oldest_first,
        with_meta,
        debug_source,
    };
    let resp = transport::send(req)?;
    emit_warnings(&resp.data);
    print_response(&resp.data, &opts)
}

pub fn parse_time(s: &str) -> Result<i64> {
    use chrono::{Local, TimeZone};
    for fmt in &["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|d| d.timestamp())
                .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0).unwrap();
        return Local
            .from_local_datetime(&dt)
            .single()
            .map(|d| d.timestamp())
            .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
    }
    anyhow::bail!(
        "无法解析时间 '{}'，支持 YYYY-MM-DD / YYYY-MM-DD HH:MM / YYYY-MM-DD HH:MM:SS",
        s
    )
}

pub fn parse_time_end(s: &str) -> Result<i64> {
    use chrono::{Local, TimeZone};
    if s.len() == 10 {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let dt = d.and_hms_opt(23, 59, 59).unwrap();
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|d| d.timestamp())
                .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
        }
    }
    parse_time(s)
}

/// 将消息类型字符串转为 local_type 整数，未知类型返回 None
pub fn parse_msg_type(s: &str) -> Option<i64> {
    match s {
        "text" => Some(1),
        "image" => Some(3),
        "voice" => Some(34),
        "video" => Some(43),
        "sticker" => Some(47),
        "location" => Some(48),
        "link" | "file" => Some(49),
        "call" => Some(50),
        "system" => Some(10000),
        _ => None,
    }
}
