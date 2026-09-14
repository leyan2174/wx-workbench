use super::output::{print_value, resolve};
use crate::ipc::Request;
use crate::service::query_client as transport;
use anyhow::Result;

pub fn cmd_members(chat: String, json: bool) -> Result<()> {
    let resp = transport::send(Request::Members { chat })?;
    let members = resp
        .data
        .get("members")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    print_value(&members, &resolve(json))
}
