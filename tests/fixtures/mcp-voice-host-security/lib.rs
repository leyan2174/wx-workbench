pub use mcp_voice_host::{config, ipc, mcp, runtime};
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/toolkit/audio/publish.rs"]
pub mod publish;
pub mod toolkit {
    pub use mcp_voice_host::toolkit_asr as asr;
    pub mod audio {
        pub use crate::publish;
        pub use mcp_voice_host::audio::*;
    }
}
pub mod asr {
    include!(concat!(env!("OUT_DIR"), "/asr.rs"));
}
pub mod mcp_voice {
    include!(concat!(env!("OUT_DIR"), "/mcp_voice.rs"));
}
pub mod cli_mcp {
    include!(concat!(env!("OUT_DIR"), "/mcp.rs"));
}
pub mod transport {
    use crate::{
        ipc::{Request, Response},
        runtime::RuntimeContext,
    };
    pub fn send_with_limits(
        runtime: &RuntimeContext,
        request: Request,
        _: std::time::Duration,
        limit: usize,
    ) -> anyhow::Result<Response> {
        eprintln!(
            "AUDIT_EVENT:ipc:{}:{}",
            runtime.id,
            serde_json::to_string(&request)?
        );
        if let Request::ResolveChat { chat } = &request {
            anyhow::ensure!(chat == "synthetic-peer", "unknown synthetic contact");
            return Ok(Response::ok(serde_json::json!({"username":chat})));
        }
        let bytes = std::fs::read(std::env::var_os("AUDIT_RESPONSE").expect("synthetic response"))?;
        anyhow::ensure!(bytes.len() <= limit, "synthetic IPC limit");
        Ok(serde_json::from_slice(&bytes)?)
    }
}
