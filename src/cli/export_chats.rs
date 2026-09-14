pub use super::operation_args::export_chats::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_export(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportChats { args: args.into() })
}
