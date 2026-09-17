use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Acquire verified database keys with explicit memory-scan authorization.
    Database(super::database_keys::Args),
    /// Acquire verified image materials with explicit memory-scan authorization.
    Image(super::image_keys::Args),
    /// Watch for verified image materials and exit when found.
    WatchImage(super::image_keys::MonitorArgs),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Database(args) => super::database_keys::cmd(args),
        Command::Image(args) => super::image_keys::cmd(args),
        Command::WatchImage(args) => super::image_keys::cmd_monitor(args),
    }
}
