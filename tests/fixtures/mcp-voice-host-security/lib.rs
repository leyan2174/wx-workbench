pub use mcp_voice_host::{ipc, mcp};
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/toolkit/audio/publish.rs"]
pub mod publish;
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod toolkit_asr;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code)]
pub mod db_cache;
#[cfg(test)]
#[path = "../../../src/crypto/test_support.rs"]
pub mod crypto_test_support;
pub mod crypto {
    pub use mcp_voice_host::crypto::*;
    #[cfg(test)]
    pub use crate::crypto_test_support as test_support;
}
#[path = "../../../src/toolkit/legacy.rs"]
#[allow(dead_code)]
pub mod legacy;
pub mod toolkit {
    pub use crate::private_file;
    pub use crate::toolkit_asr as asr;
    pub use crate::legacy;
    pub mod audio {
        pub use crate::publish;
        pub use mcp_voice_host::audio::*;
    }
}
pub mod asr {
    include!(concat!(env!("OUT_DIR"), "/asr.rs"));
}
// 共享 ASR 测试通过 CLI 路径调用本壳已引入的同一操作模块。
pub mod cli {
    pub use crate::asr;
}
pub mod mcp_voice {
    include!(concat!(env!("OUT_DIR"), "/mcp_voice.rs"));
}
#[path = "../../../src/cli/mcp.rs"]
pub mod cli_mcp;
pub mod mcp_service { include!(concat!(env!("OUT_DIR"), "/mcp_service.rs")); }
pub mod daemon {
    pub use crate::db_cache as cache;
    pub use crate::mcp_service;
    pub mod operations { pub use crate::asr; }
}
#[allow(dead_code)]
pub mod service {
    pub mod mcp { pub use crate::mcp_service::Call; }
    pub mod protocol { include!(concat!(env!("OUT_DIR"), "/service_protocol.rs")); }
    pub mod client { include!(concat!(env!("OUT_DIR"), "/service_client.rs")); }
    pub mod transport { include!(concat!(env!("OUT_DIR"), "/service_transport.rs")); }
}
#[path = "../mcp-auth/mock.rs"]
pub mod authenticated_mock;
#[allow(dead_code)]
pub mod transport { include!(concat!(env!("OUT_DIR"), "/service_query_client.rs")); }
