//! Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;
use std::path::PathBuf;

pub fn cmd_export(chat: String, output: PathBuf) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportChat { chat, output })
}
