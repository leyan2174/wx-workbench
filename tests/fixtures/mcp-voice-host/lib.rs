#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod toolkit_asr;
pub mod toolkit {
    pub use super::audio;
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
pub mod runtime;
pub mod mcp {
    pub use crate::protocol;
}
#[path = "../../../src/cli/asr.rs"]
pub mod cli_asr;
#[path = "../../../src/cli/mcp_voice.rs"]
pub mod mcp_voice;
// mcp_voice 使用与生产 cli 相同的相邻模块名称。
pub use cli_asr as asr;
