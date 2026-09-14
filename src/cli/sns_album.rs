//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::sns_album::Args;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_sns_album(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::SnsAlbum { args })
}
