// 独立安全测试直接调用生产 ASR、缓存和文件保护模块。
#[path = "../../support/asr_runtime.rs"]
#[allow(dead_code)] // 测试壳只调用部分运行时入口。
mod asr_runtime;
pub use asr_runtime::{config, crypto, runtime};
pub mod daemon {
    pub use super::asr_runtime::daemon::cache;
    pub mod operations { pub use crate::cli_asr as asr; }
}
#[path = "../../../src/application/transcription/mod.rs"]
#[allow(dead_code)] // Cache tests omit Python host-input discovery.
pub mod transcription_app;
#[path = "../../../src/infrastructure/audio/mod.rs"]
#[allow(dead_code)] // Cache tests omit MP3/WAV publication entry points.
pub mod audio;
#[path = "../../../src/infrastructure/transcription/mod.rs"]
#[allow(dead_code)] // The cache fixture exercises engines through the application pipeline.
pub mod transcription_engine;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // Audio cache tests omit image scan and metadata guards.
pub mod local_files;
pub mod attachment {
    pub use super::local_files;
}
#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // 独立 harness 只使用路径隔离；不运行其他批处理入口。
mod files;
pub mod infrastructure {
    pub use crate::audio;
    pub(crate) use crate::setup as configuration;
    pub(crate) use crate::files as publication;
    pub(crate) use crate::transcription_engine as transcription;
}
pub mod application {
    pub(crate) use crate::transcription_app as transcription;
}
pub use transcription_app::{cache, cached, transcribe_audio_bytes, Backend, Transcription};
#[cfg(test)]
pub(crate) use transcription_engine::{local, openai};
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
    pub mod protocol {
        pub use crate::asr_runtime::monitor_contract as monitor;
    }
}

#[path = "../../support/media_business.rs"]
pub mod business;
#[path = "../../support/voice_media_adapters.rs"]
pub mod adapters;

#[path = "../../support/managed_process.rs"]
pub mod windows_process;
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Cache fixture embeds the store but does not run legacy migration.
pub mod key_store;
#[path = "../../../src/infrastructure/configuration.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed by this slice.
pub mod setup;
#[path = "../../../src/private_file.rs"]
pub mod private_file;

#[path = "../../../src/ipc.rs"]
pub mod ipc;
