use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Export emoticons with saved account keys; WeChat need not be running.
    Export(super::export_emoticons::Args),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Export(args) => crate::service::operation_client::run(
            crate::service::operations::Operation::ExportEmoticons(args.into()),
        ),
    }
}
