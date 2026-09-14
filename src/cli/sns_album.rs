pub use super::operation_args::sns_album::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_sns_album(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::SnsAlbum { args: args.into() })
}
