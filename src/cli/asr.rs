//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::asr::{TranscribeAudioNativeArgs, TranscribeChatNativeArgs};
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_transcribe_audio_native(args: TranscribeAudioNativeArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::TranscribeAudio { args })
}

pub fn cmd_transcribe_chat_native(args: TranscribeChatNativeArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::TranscribeChat { args })
}
