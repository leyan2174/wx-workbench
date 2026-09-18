use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Acquire verified database keys with explicit memory-scan authorization.
    Database(super::database_keys::Args),
    /// Acquire verified image materials with explicit memory-scan authorization.
    Image(super::image_keys::Args),
    /// Import verified image material from private stdin into the selected account.
    ImportImage(super::image_keys::ImportArgs),
    /// Watch for verified image materials and exit when found.
    WatchImage(super::image_keys::MonitorArgs),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Database(args) => super::database_keys::cmd(args),
        Command::Image(args) => super::image_keys::cmd(args),
        Command::ImportImage(args) => super::image_keys::cmd_import(args),
        Command::WatchImage(args) => super::image_keys::cmd_monitor(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Invocation {
        #[command(subcommand)]
        command: Command,
    }

    #[test]
    fn image_import_is_separate_from_offline_and_memory_acquisition() {
        assert!(matches!(
            Invocation::try_parse_from(["keys", "import-image", "--stdin"])
                .unwrap()
                .command,
            Command::ImportImage(_)
        ));
        for mode in ["--offline", "--authorize-memory-scan"] {
            assert!(Invocation::try_parse_from(["keys", "image", mode]).is_ok());
            assert!(Invocation::try_parse_from(["keys", "import-image", "--stdin", mode]).is_err());
        }
    }
}
