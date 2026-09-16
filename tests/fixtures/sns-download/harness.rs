// 主线程尚未接线时，使用真实守卫与真实下载源码独立验收。
#![allow(dead_code)]
mod publication;
pub mod infrastructure {
    pub(crate) use crate::publication::setup as configuration;
    pub(crate) use crate::publication::files as publication;
}
#[path = "../../../src/private_file.rs"]
mod private_file;
pub use publication::{adapters, config, crypto, daemon, key_store, runtime, service, toolkit};
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
mod attachment {
    pub(crate) use crate::local_files;
}
#[path = "../../../src/application/moments/download.rs"]
mod download;
