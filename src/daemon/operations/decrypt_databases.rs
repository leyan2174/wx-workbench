use crate::application::database_decryption;
use anyhow::Result;

pub(super) fn execute(incremental: bool, dry_run: bool) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()?;
    let keys = crate::service::worker_keys::database_keys(&runtime)?
        .ok_or(crate::key_store::Error::Missing)?;
    super::database_key_validation::validate_paths(&runtime, &keys.0)?;
    database_decryption::decrypt(&runtime, &keys.0, incremental, dry_run)
}
