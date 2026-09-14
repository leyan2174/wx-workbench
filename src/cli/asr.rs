pub use super::operation_args::asr::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_transcribe_audio_native(args: TranscribeAudioNativeArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::TranscribeAudio { args: args.into() })
}

pub fn cmd_transcribe_chat_native(args: TranscribeChatNativeArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::TranscribeChat { args: args.into() })
}
