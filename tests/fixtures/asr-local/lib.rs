//! Standalone backend tests use the real shared Windows process supervisor.
#[path = "../../../src/windows_process/managed.rs"]
pub mod managed;
pub mod windows_process {
    pub use crate::managed;
}

#[path = "../../../src/toolkit/asr/local.rs"]
pub mod local;
#[path = "../../../src/toolkit/asr/windows_supervision.rs"]
mod windows_supervision;
pub use local::*;
