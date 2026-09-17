use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Export chats selected by dates, incremental JSON, or a plan CSV.
    Export(super::export_chats::Args),
    /// Export all chats from prepared decrypted databases.
    ExportAll(super::export_all::Args),
    /// Export personal chats as CSV/HTML/JSON with media directories.
    ExportMessages(super::export_messages::Args),
    /// Publish incremental files and a manifest, optionally appending a new batch.
    ExportDelta(super::export_delta::Args),
    /// Build a plan CSV from explicit offline databases and media directories.
    Plan(super::chat_plan::Args),
    /// Transcribe voice messages in exported chat JSON.
    Transcribe(super::asr_batch::Args),
    /// Transcribe an explicit media manifest and atomically update the chat export.
    TranscribeManifest(super::asr::TranscribeChatNativeArgs),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Export(args) => super::export_chats::cmd_export(args),
        Command::ExportAll(args) => crate::service::operation_client::run(
            crate::service::operations::Operation::ExportAll { args: args.into() },
        ),
        Command::ExportMessages(args) => super::export_messages::cmd(args),
        Command::ExportDelta(args) => super::export_delta::cmd(args),
        Command::Plan(args) => super::chat_plan::cmd(args),
        Command::Transcribe(args) => super::asr_batch::cmd(args),
        Command::TranscribeManifest(args) => super::asr::cmd_transcribe_chat_native(args),
    }
}
