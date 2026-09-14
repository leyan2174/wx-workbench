pub use super::operation_args::sns_timeline::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::SnsTimeline { args: args.into() })
}
