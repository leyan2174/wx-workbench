//! Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_init(
    force: bool,
    db_dir_override: Option<String>,
    provider: crate::scanner::KeyProvider,
    restart: bool,
    executable: Option<std::path::PathBuf>,
    timeout: u64,
) -> Result<()> {
    crate::service::operation_client::run(Operation::Initialize {
        force,
        db_dir_override,
        provider,
        restart,
        executable,
        timeout,
    })
}
