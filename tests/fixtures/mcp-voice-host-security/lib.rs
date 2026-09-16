pub use mcp_voice_host::{ipc, mcp};
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/runtime.rs"]
#[allow(dead_code)] // This fixed-account fixture omits bootstrap and other operation lifecycle entry points.
pub mod runtime;
#[path = "../../../src/private_file.rs"]
pub mod private_file;
#[path = "../../../src/infrastructure/configuration.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // Host security exercises publication guards, not directory collection.
pub mod files;
#[path = "../../../src/infrastructure/audio/wav.rs"]
pub mod audio_wav;
pub mod infrastructure {
    pub(crate) use crate::setup as configuration;
    pub(crate) use crate::files as publication;
    pub mod audio {
        pub use crate::audio_wav::validate_wav;
        pub use mcp_voice_host::audio::{
            decode_silk_to_pcm, normalize_silk, pcm24k_to_wav, prepare_wav_bytes,
        };
        pub(crate) mod publish {
            pub(crate) use crate::publish::publish_wav_noclobber;
        }
    }
    pub(crate) use crate::transcription_engine as transcription;
}
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Embedded store keeps migration/seed APIs for other harnesses.
pub mod key_store;
// Only managed execution is used here; root tests cover Frida handle isolation.
#[path = "../../support/managed_process.rs"]
pub mod windows_process;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/infrastructure/audio/publish.rs"]
pub mod publish;
#[path = "../../../src/application/transcription/mod.rs"]
pub mod transcription_app;
#[path = "../../../src/infrastructure/transcription/mod.rs"]
pub mod transcription_engine;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code, unused_imports)] // Voice-only host does not consume the ResourceSnapshot re-export.
pub mod db_cache;
#[path = "../../../src/service/monitor.rs"]
pub mod monitor_contract;
#[cfg(test)]
#[path = "../../../src/crypto/test_support.rs"]
pub mod crypto_test_support;
pub mod crypto {
    pub use mcp_voice_host::crypto::*;
    #[cfg(test)]
    pub use crate::crypto_test_support as test_support;
}
pub mod application {
    pub use crate::transcription_app as transcription;
}
pub mod asr {
    include!(concat!(env!("OUT_DIR"), "/asr.rs"));
}
// 共享 ASR 测试通过 CLI 路径调用本壳已引入的同一操作模块。
pub mod cli {
    pub use crate::operation_args;
    pub use crate::asr;
}
pub mod mcp_voice {
    include!(concat!(env!("OUT_DIR"), "/mcp_voice.rs"));
}
#[path = "../../../src/cli/mcp.rs"]
pub mod cli_mcp;
#[path = "../../../src/cli/mcp_tasks.rs"]
pub mod mcp_tasks;
pub mod mcp_service { include!(concat!(env!("OUT_DIR"), "/mcp_service.rs")); }
pub mod daemon {
    pub use crate::db_cache as cache;
    pub use crate::mcp_service;
    pub mod operations { pub use crate::asr; }
}
#[allow(dead_code)]
pub mod service {
    pub use crate::asr_contracts as operation_requests;
    pub use crate::mcp_contract as mcp;
    pub mod query_client { include!(concat!(env!("OUT_DIR"), "/service_query_client.rs")); }
    pub mod config_pin { include!(concat!(env!("OUT_DIR"), "/service_config_pin.rs")); }
    pub mod plan { include!(concat!(env!("OUT_DIR"), "/service_plan.rs")); }
    pub mod settings { include!(concat!(env!("OUT_DIR"), "/service_settings.rs")); }
    pub mod protocol {
        include!(concat!(env!("OUT_DIR"), "/service_protocol.rs"));
        pub use crate::monitor_contract as monitor;
    }
    pub mod worker_keys { include!(concat!(env!("OUT_DIR"), "/service_worker_keys.rs")); }
    pub mod client { include!(concat!(env!("OUT_DIR"), "/service_client.rs")); }
    pub mod transport { include!(concat!(env!("OUT_DIR"), "/service_transport.rs")); }
}
#[path = "../mcp-auth/mock.rs"]
pub mod authenticated_mock;
pub use service::query_client as transport;

#[path = "../../support/asr_contracts.rs"]
pub mod asr_contracts;
#[path = "../../../src/service/mcp.rs"]
pub mod mcp_contract;
#[path = "../../support/mcp_argument_parsers.rs"]
pub mod operation_args;

#[path = "../../support/media_business.rs"]
pub mod business;
#[path = "../../support/voice_media_adapters.rs"]
pub mod adapters;
