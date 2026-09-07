// 只引用真实实现；公共注册未完成时仍可独立编译，不复制生产逻辑。
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
#[path = "cli.rs"]
pub mod cli;
#[path = "../../../src/toolkit/files.rs"]
mod files;
#[path = "../../../src/toolkit/sns/video_runtime.rs"]
pub mod video;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
pub mod production_cache;
#[path = "../../../src/toolkit/legacy.rs"]
pub mod legacy;

pub mod daemon {
    pub use super::production_cache as cache;
}

pub mod toolkit {
    pub(crate) use super::files::separate;
    pub use super::{asr, audio, legacy};
}

pub mod attachment {
    pub use super::local_files;
}
