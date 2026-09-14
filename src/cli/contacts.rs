use super::output::{print_value, resolve};
use crate::ipc::Request;
use crate::service::query_client as transport;
use anyhow::Result;

pub fn cmd_contacts(query: Option<String>, limit: usize, json: bool) -> Result<()> {
    let resp = transport::send(Request::Contacts {
        query,
        limit,
        legacy_view: false,
    })?;
    let contacts = resp
        .data
        .get("contacts")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    print_value(&contacts, &resolve(json))
}
