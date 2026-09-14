pub use super::operation_args::image_keys::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeys { args: args.into() })
}

pub fn cmd_monitor(args: MonitorArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeyMonitor { args: args.into() })
}
