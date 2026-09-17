use crate::service::{operation_client, operations::Operation};
use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Command {
    /// Convert a SILK file to MP3 using ffmpeg.
    Convert {
        input: String,
        output: Option<String>,
    },
    /// Export MP3 files from a prepared media database using ffmpeg.
    Export {
        /// Explicit configuration containing decrypted_dir and output_base_dir.
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Comma-separated usernames; defaults to WECHAT_EXPORT_CONTACTS.
        #[arg(long)]
        contacts: Option<String>,
    },
    /// Transcribe SILK/WAV with a local model or an explicitly authorized provider.
    Transcribe(super::asr::TranscribeAudioNativeArgs),
    /// Transcribe a message from a static snapshot using its shard and server ID.
    TranscribeMessage(super::asr_database::TranscribeDatabaseNativeArgs),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Convert { input, output } => {
            operation_client::run(Operation::ConvertAudio { input, output })
        }
        Command::Export {
            config,
            output_dir,
            contacts,
        } => operation_client::run(Operation::ExportAudio {
            config,
            output_dir,
            contacts,
        }),
        Command::Transcribe(args) => super::asr::cmd_transcribe_audio_native(args),
        Command::TranscribeMessage(args) => {
            super::asr_database::cmd_transcribe_database_native(args)
        }
    }
}
