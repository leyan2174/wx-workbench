// 独立安全测试直接调用生产 ASR、缓存和文件保护模块。
#[path = "../../support/asr_runtime.rs"]
#[allow(dead_code)] // 测试壳只调用部分运行时入口。
mod asr_runtime;
pub use asr_runtime::{config, crypto, runtime};
pub mod daemon {
    pub use super::asr_runtime::daemon::cache;
    pub mod operations { pub use crate::cli_asr as asr; }
}
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
pub mod attachment {
    pub use super::local_files;
}
#[path = "../../../src/toolkit/files.rs"]
#[allow(dead_code)] // 独立 harness 只使用路径隔离；不运行其他批处理入口。
mod files;
pub mod toolkit {
    pub use super::asr_runtime::legacy;
    pub(crate) use super::files::separate;
    pub use super::{asr, audio};
}
pub use asr::{cache, cached, local, openai, transcribe_audio_bytes, Backend, Transcription};
#[path = "../../../src/daemon/operations/asr.rs"]
pub mod cli_asr;
pub mod cli;

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod cli_path_tests;
#[cfg(test)]
mod security_tests;

#[path = "../../support/asr_contracts.rs"]
pub mod asr_contracts;
pub mod service {
    pub use super::asr_contracts as operation_requests;
}
