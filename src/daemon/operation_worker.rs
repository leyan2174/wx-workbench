//! Internal typed worker. Only the daemon owns its suspended start and Windows Job.
use anyhow::{ensure, Context, Result};
use std::io::Read;

pub(crate) fn run() -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut length = [0u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(
        length > 0 && length <= crate::service::protocol::MAX_REQUEST_BYTES,
        "Invalid operation frame"
    );
    let mut bytes = zeroize::Zeroizing::new(vec![0u8; length]);
    input.read_exact(&mut bytes)?;
    let operation = serde_json::from_slice(&bytes).context("Invalid typed operation")?;
    let mut trailing = [0u8; 1];
    ensure!(
        input.read(&mut trailing)? == 0,
        "Unexpected operation input"
    );
    drop(input);
    if let Ok(expected) = std::env::var("WX_CLI_EXPECTED_RUNTIME") {
        ensure!(
            crate::runtime::RuntimeContext::load()?.id == expected,
            "Account changed before operation execution"
        );
    }
    crate::daemon::operations::execute(operation)
}
