// 编译完整生产模块；integration test 避免引入与本 fixture 无关的 CLI 单元测试。
#[path = "../../support/asr_runtime.rs"]
#[allow(dead_code)] // WAV 测试不调用后台管理入口。
mod asr_runtime;
pub use asr_runtime::{config, crypto, daemon, runtime};
#[path = "../../../src/toolkit/asr/mod.rs"]
#[allow(dead_code)] // WAV publication does not discover Python host inputs.
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
#[allow(dead_code)] // WAV publication does not invoke the MP3 checked wrapper.
pub mod audio;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // WAV fixture 不调用图片目录扫描和图片密钥读取。
pub mod local_files;

pub mod toolkit {
    pub(crate) use super::files::{validate_export_target, ExportTarget};
    pub use super::asr_runtime::legacy;
    pub use super::{asr, audio, setup, private_file};
}
pub mod attachment {
    pub use super::local_files;
}

mod cases;
pub use cases::run_suite;
#[path = "../../support/media_business.rs"]
pub mod business;
#[path = "../../support/voice_media_adapters.rs"]
pub mod adapters;

#[path = "../../support/managed_process.rs"]
pub mod windows_process;
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // WAV fixture retains store APIs but has no legacy migration entry point.
pub mod key_store;
#[path = "../../../src/toolkit/setup.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;

#[path = "../../../src/toolkit/files.rs"]
#[allow(dead_code)] // WAV publication exercises target guards, not directory collection.
mod files;
