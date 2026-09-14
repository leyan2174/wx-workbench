//! Authenticated MCP transport and host policy; never tool-supplied settings.
use crate::{
    ipc::{Request, Response},
    mcp::protocol::{CallBudget, DispatchError},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::service::operation_requests::asr::BackendArgs;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    pub backend: BackendArgs,
    /// 显式主机转录缓存文件；省略时不持久化转录缓存。
    pub voice_cache_file: Option<PathBuf>,
}

/// Constructed from process startup arguments, never from tools/call arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSettings {
    pub media_output_root: Option<PathBuf>,
    pub image_key_file: Option<PathBuf>,
    pub configured_local_python: bool,
    pub voice: VoiceSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub session: String,
    /// False after the first reply: a daemon restart must not silently rebind a session.
    pub open_session: bool,
    pub owner_pid: u32,
    pub runtime_id: String,
    pub host: HostSettings,
    pub budget: CallBudget,
    /// None closes the session without touching the account or query state.
    pub request: Option<Box<Request>>,
}

pub fn unpack(response: Response) -> Result<Response, DispatchError> {
    if !response.ok || response.error.is_some() {
        return Err(DispatchError::Unavailable);
    }
    serde_json::from_value::<Result<Response, DispatchError>>(response.data)
        .map_err(|_| DispatchError::InvalidResponse)?
}
