//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::image_keys::{Args, MonitorArgs};
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeys { args })
}

pub fn cmd_monitor(args: MonitorArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeyMonitor { args })
}
