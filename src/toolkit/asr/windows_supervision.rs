//! ASR diagnostics over the shared Windows child lifecycle; protocols stay in backends.
use crate::windows_process::managed;
use anyhow::{Context, Result};
use std::{io::Read, os::windows::io::AsRawHandle, process::Child};

#[derive(Clone, Copy)]
pub(super) enum Caller {
    WhisperCpp,
    Python,
}

impl Caller {
    fn label(self) -> &'static str {
        match self {
            Self::WhisperCpp => "ASR",
            Self::Python => "inference",
        }
    }
}

pub(super) struct Job(managed::Job);

impl Job {
    /// The caller must use SUSPENDED_NO_WINDOW; no helper code runs before assignment.
    pub(super) fn attach(child: &Child, caller: Caller) -> Result<Self> {
        managed::Job::attach_suspended(child)
            .map(Self)
            .with_context(|| format!("attach {} Job Object failed", caller.label()))
    }
}

pub(super) fn stop(child: &mut Child, job: Option<&Job>) -> Result<()> {
    managed::stop(child, job.map(|job| &job.0))
}

pub(super) fn drain<R: Read + AsRawHandle>(
    pipe: &mut R,
    total: &mut u64,
    limit: u64,
    caller: Caller,
) -> Result<()> {
    let result = managed::drain(pipe, total, limit, None);
    if *total > limit {
        anyhow::bail!(
            "{}",
            match caller {
                Caller::WhisperCpp => "ASR process stream limit exceeded",
                Caller::Python => "local inference process output limit exceeded; output withheld",
            }
        );
    }
    result.map(|_| ()).with_context(|| {
        format!(
            "{} process output supervision failed; output withheld",
            caller.label()
        )
    })
}
