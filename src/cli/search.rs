use super::output::{emit_warnings, print_response, OutputOpts};
use crate::ipc::Request;
use crate::service::message_filter::cli_type as parse_msg_type;
use crate::service::query_client as transport;
use crate::service::time::{parse_time, parse_time_end};
use anyhow::Result;

pub fn cmd_search(
    keyword: String,
    chats: Vec<String>,
    limit: usize,
    since: Option<String>,
    until: Option<String>,
    msg_type: Option<String>,
    opts: OutputOpts,
) -> Result<()> {
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;
    let type_val = msg_type.as_deref().and_then(parse_msg_type);
    let chats_opt = if chats.is_empty() { None } else { Some(chats) };
    let (with_meta, debug_source) = opts.request_flags();

    let req = Request::Search {
        keyword,
        chats: chats_opt,
        limit,
        since: since_ts,
        until: until_ts,
        msg_type: type_val,
        with_meta,
        debug_source,
    };

    let resp = transport::send(req)?;
    emit_warnings(&resp.data);
    print_response(&resp.data, &opts)
}
