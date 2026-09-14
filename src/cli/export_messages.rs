//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::export_messages::Args;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ExportMessages { args })
}
