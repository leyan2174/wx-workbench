// 只引用真实实现；公共注册未完成时仍可独立编译，不复制生产逻辑。
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "cli.rs"]
pub mod cli;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/toolkit/files.rs"]
mod files;
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/key_store/mod.rs"]
pub mod key_store;
#[path = "../../../src/toolkit/legacy.rs"]
pub mod legacy;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/daemon/cache.rs"]
#[allow(unused_imports)] // 语音和视频测试不调用图片资源快照接口。
pub mod production_cache;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/toolkit/setup.rs"]
pub mod setup;
#[path = "../../../src/toolkit/sns/video_runtime.rs"]
pub mod video;
#[path = "../../../src/windows_process.rs"]
pub mod windows_process;

pub mod daemon {
    pub mod operations {
        pub use super::super::cli::{asr, asr_database};
    }
    pub use super::production_cache as cache;
}

pub mod toolkit {
    pub(crate) use super::files::{separate, validate_export_target};
    pub use super::{asr, audio, legacy};
    pub use super::{private_file, setup};
}

pub mod attachment {
    pub use super::local_files;
}

#[path = "../../support/asr_contracts.rs"]
pub mod asr_contracts;
pub mod service {
    pub use super::asr_contracts as operation_requests;
}
