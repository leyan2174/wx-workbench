// 主线程尚未接线时，使用真实守卫与真实下载源码独立验收。
#![allow(dead_code)]
mod publication;
pub use publication::{config, crypto, daemon, key_store, runtime, toolkit};
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/toolkit/sns/download.rs"]
mod download;
