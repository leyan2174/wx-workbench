//! Argument adaptation and authenticated daemon operation forwarding only.
use super::output::OutputOpts;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_export(
    chat: String,
    since: Option<String>,
    until: Option<String>,
    limit: usize,
    format: String,
    output: Option<String>,
    opts: OutputOpts,
) -> Result<()> {
    crate::service::operation_client::run(Operation::Export {
        chat,
        since,
        until,
        limit,
        format,
        output,
        opts,
    })
}
