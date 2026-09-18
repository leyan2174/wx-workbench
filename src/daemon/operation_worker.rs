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
    let request: crate::service::worker_keys::Input<crate::service::operations::Operation> =
        serde_json::from_slice(&bytes).context("Invalid typed operation")?;
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
    let access = crate::service::worker_keys::install(request.access)?;
    let result = crate::daemon::operations::execute(request.operation);
    drop(access);
    finish(result)
}

/// Worker adapter only: domain work has returned, so its guards have already unwound.
pub(crate) fn finish(result: Result<()>) -> Result<()> {
    if let Err(error) = &result {
        if let Some(failure) = error.downcast_ref::<crate::ipc::outcome::BusinessFailure>() {
            eprintln!("{}", failure.public_message());
            std::process::exit(failure.0.worker_exit_code());
        }
    }
    result
}

#[cfg(test)]
mod outcome_tests {
    use super::*;
    use crate::{ipc::outcome::BusinessOutcome, windows_process::managed};
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    #[ignore = "Synthetic business exit adapter fixture"]
    fn worker_business_exit_fixture() {
        let code = std::env::var("WX_TEST_BUSINESS_EXIT")
            .unwrap()
            .parse()
            .unwrap();
        let result = BusinessOutcome::from_worker_exit(code)
            .require_success()
            .map_err(anyhow::Error::new)
            .context("SYNTHETIC_PRIVATE_KEY");
        finish(result).unwrap();
    }

    #[test]
    fn actual_worker_exit_codes_distinguish_partial_and_refusal_without_details() {
        let module = module_path!().split_once("::").unwrap().1;
        let fixture = format!("{module}::worker_business_exit_fixture");
        for outcome in [
            BusinessOutcome::Partial,
            BusinessOutcome::Refused,
            BusinessOutcome::Failure,
        ] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", fixture.as_str(), "--ignored", "--nocapture"])
                .env(
                    "WX_TEST_BUSINESS_EXIT",
                    outcome.worker_exit_code().to_string(),
                );
            let output = managed::output(
                &mut command,
                true,
                Instant::now() + Duration::from_secs(5),
                64 * 1024,
                || false,
            )
            .unwrap();
            assert_eq!(output.status.code(), Some(outcome.worker_exit_code()));
            assert!(String::from_utf8_lossy(&output.stderr).contains(outcome.public_message()));
            assert!(!String::from_utf8_lossy(&output.stderr).contains("SYNTHETIC_PRIVATE_KEY"));
        }
    }
}
