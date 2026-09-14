#![cfg(windows)]
#![allow(dead_code)]
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/toolkit/directory_publish/mod.rs"]
mod publish;
