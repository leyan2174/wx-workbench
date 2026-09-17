use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Decrypt database main files with saved account keys; does not merge WAL.
    Decrypt {
        #[arg(short = 'i', long)]
        incremental: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Decrypt {
            incremental,
            dry_run,
        } => crate::service::operation_client::run(
            crate::service::operations::Operation::DecryptDatabases {
                incremental,
                dry_run,
            },
        ),
    }
}
