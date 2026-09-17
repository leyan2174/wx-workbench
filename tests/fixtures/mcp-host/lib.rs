#[path = "../../../src/infrastructure/publication.rs"]
#[allow(dead_code)] // MCP host exercises publication guards, not directory collection.
pub mod files;
#[path = "../../../src/private_file.rs"]
pub mod private_file;
#[path = "../../../src/infrastructure/configuration.rs"]
#[allow(dead_code)]
// Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
pub mod infrastructure {
    pub(crate) use crate::files as publication;
    pub(crate) use crate::setup as configuration;
}
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)]
// Embedded store retains legacy import/seed APIs; this host does not run migration.
pub mod key_store;
// Only managed execution is used here; root tests cover Frida handle isolation.
#[path = "../../../src/service/config_pin.rs"]
pub mod config_pin;
#[path = "../../../src/service/message_filter.rs"]
pub mod message_filter;
#[path = "../../../src/service/monitor.rs"]
pub mod monitor_contract;
#[path = "../../support/managed_process.rs"]
pub mod windows_process;
pub mod service {
    pub use crate::config_pin;
    pub use crate::mcp_contract as mcp;
    pub use crate::message_filter;
    pub mod protocol {
        pub use crate::monitor_contract as monitor;
    }
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
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code, unused_imports)]
// 独立 MCP 宿主不使用图片资源快照等缓存入口；生产根使用该导出，缓存测试仍保留。
pub mod db_cache;
#[path = "../../../src/daemon/mcp_service.rs"]
pub mod mcp_service;
pub mod daemon {
    pub use crate::db_cache as cache;
    pub use crate::mcp_service;
}

pub fn fixture_runtime(
    config: &std::path::Path,
    home: &std::path::Path,
) -> anyhow::Result<runtime::RuntimeContext> {
    runtime::RuntimeContext::from_config(
        config.to_owned(),
        config::load_config_at(config)?,
        home.to_owned(),
    )
}

#[path = "../../../src/service/mcp.rs"]
pub mod mcp_contract;

#[path = "../../support/voice_media_adapters.rs"]
pub mod adapters;
#[path = "../../support/media_business.rs"]
pub mod business;
