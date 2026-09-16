//! Standalone backend tests use the real shared Windows process supervisor.
#[path = "../../../src/windows_process/managed.rs"]
#[allow(dead_code)] // Local ASR needs managed execution, not account-capture attachment APIs.
pub mod managed;
pub mod windows_process {
    pub use crate::managed;
}

#[path = "../../../src/infrastructure/transcription/local.rs"]
pub mod local;
#[path = "../../../src/infrastructure/transcription/windows_supervision.rs"]
#[allow(dead_code)] // This backend fixture exercises C++ supervision, not the Python variant.
mod windows_supervision;
pub use local::*;
