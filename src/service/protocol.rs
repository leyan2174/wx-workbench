//! Versioned local IPC. Only Configure may introduce allowed input paths.
pub use super::settings::SettingsInput;
#[path = "monitor.rs"]
pub(crate) mod monitor;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
pub const VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// 提交幂等键就是后台任务 ID；各入口使用同一格式，不能自行截短或重新生成。
pub fn valid_task_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Kind {
    WechatKeys,
    WechatDecrypt,
    ImageKey,
    ExportAll,
    DecodeImages,
    SnsDecrypt,
}

pub fn parse_task_kind(value: &str) -> Result<Kind, String> {
    serde_json::from_value(Value::String(value.into()))
        .map_err(|_| format!("Unknown task kind: {value}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Format {
    Json,
    Csv,
    Html,
}
impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Html => "html",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub users: Vec<String>,
    pub formats: Vec<Format>,
    pub include_sns: bool,
    pub include_sns_media: bool,
    pub include_images: bool,
    pub allow_missing_media: bool,
    pub authorize_memory_scan: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            users: Vec::new(),
            formats: Vec::new(),
            include_sns: false,
            include_sns_media: false,
            include_images: true,
            allow_missing_media: false,
            authorize_memory_scan: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Submission {
    pub kind: Kind,
    #[serde(default)]
    pub options: Options,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Log {
    pub seq: u64,
    pub stream: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub options: Options,
    pub status: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub logs: std::collections::VecDeque<Log>,
    pub log_start_seq: u64,
    pub next_log_seq: u64,
    pub output_dir: PathBuf,
    pub error: Option<String>,
}
impl Task {
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "succeeded" | "failed" | "cancelled" | "interrupted"
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Call {
    WorkerKeyRevision {
        request: super::worker_keys::RevisionRequest,
    },
    WorkerKeys {
        request: super::worker_keys::UpdateRequest,
    },
    WorkerDatabaseKeys {
        request: super::worker_keys::DatabaseReadRequest,
    },
    WorkerImageMaterial {
        request: super::worker_keys::ImageReadRequest,
    },
    Monitor {
        request: monitor::Call,
    },
    Mcp {
        request: Box<super::mcp::Call>,
    },
    Web {
        request: Box<super::web::Call>,
    },
    OperationStart {
        id: String,
        invocation: Box<super::operation_protocol::Invocation>,
    },
    OperationPoll {
        id: String,
        after: u64,
    },
    OperationCancel {
        id: String,
    },
    Configure {
        settings: SettingsInput,
    },
    Info {},
    Submit {
        idempotency_key: String,
        task: Submission,
    },
    List {},
    Get {
        id: String,
    },
    Cancel {
        id: String,
    },
    Events {
        after: u64,
        limit: usize,
        wait_ms: u64,
    },
    Shutdown {},
}
impl Call {
    pub(crate) fn response_limit(&self) -> usize {
        match self {
            Self::WorkerDatabaseKeys { .. } => super::worker_keys::MAX_DATABASE_REPLY_BYTES,
            Self::WorkerImageMaterial { .. } => super::worker_keys::MAX_IMAGE_REPLY_BYTES,
            Self::Monitor { request } => request.response_limit(),
            Self::Web { .. } => super::web::MAX_RESPONSE_BYTES,
            Self::Mcp { request } => request
                .budget
                .max_response_bytes
                .saturating_add(64 * 1024)
                .min(24 * 1024 * 1024),
            _ => MAX_RESPONSE_BYTES,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub runtime_id: String,
    pub token: String,
    pub request: Call,
}
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("version", &self.version)
            .field("runtime_id", &self.runtime_id)
            .field("token", &"[redacted]")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u32,
    pub runtime_id: String,
    pub ok: bool,
    pub data: Value,
    pub error: Option<ServiceError>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceError {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ServiceError {}
impl ServiceError {
    /// Use fixed messages, never underlying errors containing paths or secrets.
    pub fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn unauthorized() -> Self {
        Self::new("unauthorized", "Service authentication failed")
    }
}
impl Reply {
    pub fn success(runtime_id: impl Into<String>, data: Value) -> Self {
        Self {
            version: VERSION,
            runtime_id: runtime_id.into(),
            ok: true,
            data,
            error: None,
        }
    }
    pub fn failure(runtime_id: impl Into<String>, error: ServiceError) -> Self {
        Self {
            version: VERSION,
            runtime_id: runtime_id.into(),
            ok: false,
            data: Value::Null,
            error: Some(error),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub seq: u64,
    pub name: String,
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsPage {
    pub events: Vec<Event>,
    pub cursor: u64,
    pub reset: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_unknown_fields_at_every_public_boundary() {
        for value in [
            json!({"kind":"shell"}),
            json!({"kind":"voice_mp3"}),
            json!({"kind":"export_all","options":{"with_transcriptions":true}}),
            json!({"kind":"export_all","options":{"allow_upload":true}}),
            json!({"kind":"export_all","options":{"include_voice":true}}),
            json!({"kind":"export_all","path":"other"}),
            json!({"kind":"export_all","options":{"command":"cmd.exe"}}),
            json!({"kind":"wxwork_decrypt"}),
            json!({"kind":"wxwork_export"}),
            json!({"kind":"wxwork_discover"}),
            json!({"kind":"wxwork_scan"}),
            json!({"kind":"wxwork_run"}),
            json!({"kind":"export_all","options":{"all_conversations":true}}),
        ] {
            assert!(serde_json::from_value::<Submission>(value).is_err());
        }
        assert!(serde_json::from_value::<Call>(json!({"op":"info","path":"other"})).is_err());
        assert!(serde_json::from_value::<Envelope>(json!({
            "version":1,"runtime_id":"r","token":"t","request":{"op":"info"},"extra":true
        }))
        .is_err());
        for value in [
            json!({"port":80}),
            json!({"open":true}),
            json!({"command":"cmd.exe"}),
            json!({"enterprise_snapshot":"removed"}),
            json!({"enterprise_data_dir":"removed"}),
        ] {
            assert!(serde_json::from_value::<SettingsInput>(value).is_err());
        }
    }
    #[test]
    fn submit_roundtrip_has_no_paths() {
        let call = Call::Submit {
            idempotency_key: "retry".into(),
            task: Submission {
                kind: Kind::WechatDecrypt,
                options: Options::default(),
            },
        };
        let bytes = serde_json::to_vec(&call).unwrap();
        assert!(matches!(
            serde_json::from_slice::<Call>(&bytes).unwrap(),
            Call::Submit { .. }
        ));
    }
}
