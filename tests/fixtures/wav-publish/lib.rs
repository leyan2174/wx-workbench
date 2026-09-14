// 编译完整生产模块；integration test 避免引入与本 fixture 无关的 CLI 单元测试。
#[path = "../../support/asr_runtime.rs"]
#[allow(dead_code)] // WAV 测试不调用后台管理入口。
mod asr_runtime;
pub use asr_runtime::{config, crypto, daemon, runtime};
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // WAV fixture 不调用图片目录扫描和图片密钥读取。
pub mod local_files;

pub mod toolkit {
    pub use super::asr_runtime::legacy;
    pub use super::{asr, audio};
}
pub mod attachment {
    pub use super::local_files;
}

mod cases;
pub use cases::run_suite;
