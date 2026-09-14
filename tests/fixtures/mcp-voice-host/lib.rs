#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/toolkit/legacy.rs"]
#[allow(dead_code)]
pub mod legacy;
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod toolkit_asr;
#[path = "../../../src/toolkit/setup.rs"]
pub mod setup;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/toolkit/files.rs"]
pub mod files;
#[path = "../../../src/key_store/mod.rs"]
pub mod key_store;
#[path = "../../../src/windows_process.rs"]
pub mod windows_process;
#[path = "../../../src/service/config_pin.rs"]
pub mod config_pin;
pub mod service {
    pub use crate::asr_contracts as operation_requests;
    pub use crate::mcp_contract as mcp;
    pub use crate::config_pin;
}
pub mod toolkit {
    pub use super::{setup, private_file};
    pub(crate) use super::files::{separate, validate_export_target};
    pub use super::audio;
    pub use super::legacy;
    pub use super::toolkit_asr as asr;
}
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // 独立宿主不使用图片目录枚举和图片密钥读取入口。
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/mcp/protocol.rs"]
#[allow(unfulfilled_lint_expectations)] // 生产二进制私有入口在此作为公开测试 API 引用。
pub mod protocol;
#[path = "../../../src/runtime.rs"]
#[allow(dead_code)]
pub mod runtime;
pub mod mcp {
    pub use crate::protocol;
}
#[path = "../../../src/daemon/operations/asr.rs"]
pub mod cli_asr;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code, unused_imports)] // 独立 voice 宿主不使用图片资源快照等缓存入口；生产根使用该导出，缓存测试仍保留。
pub mod db_cache;
#[path = "../../../src/daemon/mcp_service.rs"]
pub mod mcp_service;
pub mod daemon {
    pub use crate::db_cache as cache;
    pub use crate::mcp_service;
    pub mod operations {
        pub use crate::cli_asr as asr;
    }
}
pub mod cli {
    pub use crate::operation_args;
    pub use crate::cli_asr as asr;
}
pub use cli_asr as asr;
pub use mcp_service::voice as mcp_voice;

pub fn fixture_runtime(config: &std::path::Path, home: &std::path::Path) -> anyhow::Result<runtime::RuntimeContext> {
    runtime::RuntimeContext::from_config(config.to_owned(), config::load_config_at(config)?, home.to_owned())
}

#[path = "../../support/asr_contracts.rs"]
pub mod asr_contracts;
#[path = "../../../src/service/mcp.rs"]
pub mod mcp_contract;
#[path = "../../support/mcp_argument_parsers.rs"]
pub mod operation_args;
