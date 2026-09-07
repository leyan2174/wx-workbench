// 引用真实宿主与音频后端；不引入整个 toolkit、WASM 或后台查询器。
#[path = "../../../src/cli/asr.rs"]
pub mod asr;
#[path = "../../../src/toolkit/audio/mod.rs"]
pub mod audio;
#[path = "../../../src/cli/mcp.rs"]
pub mod cli_mcp;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // 此夹具不执行图片目录扫描。
pub mod local_files;
#[path = "../../../src/mcp/mod.rs"]
#[allow(unfulfilled_lint_expectations)] // 夹具公开协议模块，生产私有入口的 dead_code 预期不适用。
pub mod mcp;
#[path = "../../../src/cli/mcp_voice.rs"]
pub mod mcp_voice;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
#[path = "../../../src/toolkit/asr/mod.rs"]
pub mod toolkit_asr;
#[path = "../../../src/cli/transport.rs"]
pub mod transport;

pub mod toolkit {
    pub use super::{audio, toolkit_asr as asr};
}
pub mod attachment {
    pub use super::local_files;
}
pub mod cli {
    pub use super::{asr, cli_mcp as mcp, mcp_voice, transport};
}
