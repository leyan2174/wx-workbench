use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Export the selected account's locally cached timeline.
    Export(super::sns_timeline::Args),
    /// Export JSON/HTML from an explicit snapshot, offline unless authorized.
    ExportSnapshot(Box<super::export_sns::Args>),
    /// Archive all cached Moments images, including images without a cached post.
    Archive(super::sns_archive::Args),
}

pub fn cmd(command: Command) -> Result<()> {
    match command {
        Command::Export(args) => super::sns_timeline::cmd(args),
        Command::Archive(args) => super::sns_archive::cmd(args),
        Command::ExportSnapshot(args) => {
            let super::export_sns::Args {
                sns_db,
                output_dir,
                contact_db,
                contacts,
                utc_offset,
                download_media,
                update,
                adopt_existing,
                local_cache,
            } = *args;
            crate::service::operation_client::run(
                crate::service::operations::Operation::ExportMomentSnapshot {
                    sns_db,
                    output_dir,
                    contact_db,
                    contacts,
                    utc_offset,
                    download_media,
                    update,
                    adopt_existing,
                    local_cache: local_cache.into(),
                },
            )
        }
    }
}
