use crate::service::{operation_client, operations::Operation};
use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Command {
    /// Decode local image files.
    Image {
        #[command(subcommand)]
        cmd: ImageCommand,
    },
    /// Decode local Moments video files.
    Video {
        #[command(subcommand)]
        cmd: VideoCommand,
    },
}

#[derive(Subcommand)]
pub enum ImageCommand {
    /// Decode one DAT image using the selected account's saved materials.
    Decode {
        dat_file: String,
        output_file: Option<String>,
    },
    /// Decode a directory; output must be outside the source, existing files are skipped.
    DecodeDirectory {
        input_dir: String,
        output_dir: Option<String>,
    },
    /// Decode an attachment cache; output must be outside the source, no parent traversal.
    DecodeCache {
        #[arg(long)]
        attach_dir: Option<String>,
        #[arg(long)]
        decoded_dir: Option<String>,
        #[arg(long)]
        aes_key: Option<String>,
        #[arg(long)]
        xor_key: Option<String>,
        /// Decode again even when output exists.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum VideoCommand {
    /// Decode offline; checks the MP4 header, not playback validity.
    Decode {
        input: PathBuf,
        /// New output file; existing files are never overwritten.
        output: PathBuf,
        /// UTF-8 key file; unnecessary for plaintext MP4.
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Optional module whose hash must match the audited embedded module.
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
}

pub fn cmd(command: Command) -> Result<()> {
    let operation = match command {
        Command::Image {
            cmd:
                ImageCommand::Decode {
                    dat_file,
                    output_file,
                },
        } => Operation::DecodeImage {
            dat_file,
            output_file,
        },
        Command::Image {
            cmd:
                ImageCommand::DecodeDirectory {
                    input_dir,
                    output_dir,
                },
        } => Operation::DecodeImageDirectory {
            input_dir,
            output_dir,
        },
        Command::Image {
            cmd:
                ImageCommand::DecodeCache {
                    attach_dir,
                    decoded_dir,
                    aes_key,
                    xor_key,
                    force,
                },
        } => Operation::DecodeImageCache {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        },
        Command::Video {
            cmd:
                VideoCommand::Decode {
                    input,
                    output,
                    key_file,
                    wasm,
                },
        } => Operation::DecodeMomentVideo {
            input,
            output,
            key_file,
            wasm,
        },
    };
    operation_client::run(operation)
}
