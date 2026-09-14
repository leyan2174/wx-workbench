pub use super::operation_args::export_delta::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportDelta { args: args.into() })
}
