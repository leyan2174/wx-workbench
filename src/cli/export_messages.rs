pub use super::operation_args::export_messages::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportMessages { args: args.into() })
}
