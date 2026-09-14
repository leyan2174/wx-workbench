use super::output::{emit_warnings, print_response, OutputOpts};
use super::transport;
use crate::ipc::Request;
pub use crate::service::time::{parse_time, parse_time_end};
use anyhow::Result;

#[derive(clap::Args)]
pub struct Args {
    /// 聊天对象名称（支持模糊匹配）
    pub chat: String,
    /// 消息数量
    #[arg(short = 'n', long, default_value = "50")]
    pub limit: usize,
    /// 分页偏移
    #[arg(long, default_value = "0")]
    pub offset: usize,
    /// 起始时间 YYYY-MM-DD
    #[arg(long)]
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    #[arg(long)]
    pub until: Option<String>,
    /// 消息类型过滤 [text|image|voice|video|sticker|location|link|file|call|system]
    #[arg(long = "type", value_name = "TYPE",
          value_parser = ["text","image","voice","video","sticker","location","link","file","call","system"])]
    pub msg_type: Option<String>,
    /// 多类型筛选，支持逗号分隔或重复指定
    #[arg(long = "types", value_delimiter = ',', conflicts_with = "msg_type",
          value_parser = ["text","image","voice","video","sticker","location","link","file","call","system"])]
    pub msg_types: Vec<String>,
    /// 从全部分片中的最早消息开始分页
    #[arg(long)]
    pub oldest_first: bool,
    /// 输出 JSON（默认 YAML）
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_history(args: Args, opts: OutputOpts) -> Result<()> {
    let Args {
        chat,
        limit,
        offset,
        since,
        until,
        msg_type,
        msg_types,
        oldest_first,
        ..
    } = args;
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
