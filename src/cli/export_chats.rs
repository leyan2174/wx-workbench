//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::export_chats::Args;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_export(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportChats { args })
}
