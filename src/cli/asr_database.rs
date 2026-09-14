pub use super::operation_args::asr_database::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_transcribe_database_native(args: TranscribeDatabaseNativeArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::TranscribeDatabase { args: args.into() })
}
