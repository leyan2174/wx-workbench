//! Argument adaptation and authenticated daemon operation forwarding only.
use super::output::OutputOpts;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_new_messages(limit: usize, opts: OutputOpts) -> Result<()> {
    crate::service::operation_client::run(Operation::NewMessages { limit, opts })
}
