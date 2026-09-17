// 只引用真实实现；独立编译生产模块，不复制生产逻辑。
#[path = "../../../src/infrastructure/audio/mod.rs"]
pub mod audio;
#[path = "cli.rs"]
pub mod cli;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // ASR/video uses path guards and separate, not directory collection.
pub(crate) mod files;
#[path = "../../../src/application/transcription/mod.rs"]
pub mod transcription_app;
#[path = "../../../src/infrastructure/transcription/mod.rs"]
pub mod transcription_engine;
pub mod infrastructure {
    pub use super::audio;
    pub(crate) use super::files as publication;
    pub(crate) use super::setup as configuration;
    pub(crate) use super::transcription_engine as transcription;
}
pub mod application {
    pub(crate) use super::transcription_app as transcription;
}
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Embedded store retains migration/seed APIs beyond this ASR/video fixture.
pub mod key_store;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
#[path = "../../../src/daemon/cache.rs"]
#[allow(unused_imports)] // 语音和视频测试不调用图片资源快照接口。
pub mod production_cache;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/infrastructure/configuration.rs"]
pub mod setup;
#[path = "../../../src/adapters/wechat/media/sns_keystream.rs"]
pub mod video;
#[path = "../../support/managed_process.rs"]
pub mod windows_process;

pub mod daemon {
    pub mod operations {
        pub use super::super::cli::{asr, asr_database};
    }
    pub use super::production_cache as cache;
}

pub mod attachment {
    pub use super::local_files;
}

#[path = "../../support/asr_contracts.rs"]
pub mod asr_contracts;
#[path = "../../../src/service/monitor.rs"]
pub mod monitor_contract;
pub mod service {
    pub use super::asr_contracts as operation_requests;
    pub mod protocol {
        pub use super::super::monitor_contract as monitor;
    }
}

#[path = "../../support/voice_media_adapters.rs"]
pub mod adapters;
#[path = "../../support/media_business.rs"]
pub mod business;
