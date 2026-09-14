#[derive(Clone, Debug, clap::Args)]
pub struct Args {
    /// Explicitly retain account-bound material lacking local verification evidence.
    #[arg(long)]
    pub allow_unverified: bool,
    /// Remove legacy image fields after verified publication; shared files are retained.
    #[arg(long)]
    pub cleanup_legacy: bool,
}

impl From<Args> for crate::service::operation_requests::key_migration::Args {
    fn from(value: Args) -> Self {
        Self {
            allow_unverified: value.allow_unverified,
            cleanup_legacy: value.cleanup_legacy,
        }
    }
}

impl From<crate::service::operation_requests::key_migration::Args> for Args {
    fn from(value: crate::service::operation_requests::key_migration::Args) -> Self {
        Self {
            allow_unverified: value.allow_unverified,
            cleanup_legacy: value.cleanup_legacy,
        }
    }
}
