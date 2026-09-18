//! Fixed-account chat export results and bounded, identity-only artifact reads.
use serde::{Deserialize, Serialize};

pub const CHUNK_BYTES: u32 = 1024 * 1024;
pub const MAX_LIST_ITEMS: u32 = 100;
pub const MAX_ARTIFACTS: usize = 20_000;
pub const MAX_INDEX_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_MEDIA_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_TOTAL_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_TOTAL_MEDIA_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_SAFE_INTEGER: u64 = (1u64 << 53) - 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportOutcome {
    Success,
    Partial,
    Failure,
    Refused,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportAllResult {
    pub version: u32,
    pub scope: String,
    pub dry_run: bool,
    pub finalized: bool,
    pub outcome: Option<ExportOutcome>,
    pub planned_chats: Option<u64>,
    pub exported_chats: u64,
    pub failed_chats: u64,
    pub messages: u64,
    pub media_issues: u64,
    pub artifact_count: u64,
    pub artifacts_complete: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl ExportAllResult {
    pub(crate) fn empty(dry_run: bool) -> Self {
        Self {
            version: 1,
            scope: "chat_directory".into(),
            dry_run,
            finalized: false,
            outcome: None,
            planned_chats: None,
            exported_chats: 0,
            failed_chats: 0,
            messages: 0,
            media_issues: 0,
            artifact_count: 0,
            artifacts_complete: false,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn validate(&self) -> bool {
        self.version == 1
            && self.scope == "chat_directory"
            && self.artifact_count <= MAX_ARTIFACTS as u64
            && self.planned_chats.is_none_or(|v| v <= MAX_SAFE_INTEGER)
            && [
                self.exported_chats,
                self.failed_chats,
                self.messages,
                self.media_issues,
            ]
            .iter()
            .all(|v| *v <= MAX_SAFE_INTEGER)
            && self.diagnostics.len() <= 16
            && self.diagnostics.iter().all(|d| {
                d.count <= MAX_SAFE_INTEGER
                    && matches!(
                        d.code.as_str(),
                        "chat_export_failed"
                            | "media_unavailable"
                            | "artifact_limit_exceeded"
                            | "result_unavailable"
                            | "artifact_unsafe"
                            | "artifact_changed"
                            | "artifact_unavailable"
                            | "export_interrupted"
                            | "artifact_finalization_timeout"
                    )
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub artifact_id: String,
    pub name: String,
    pub role: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactsPage {
    pub version: u32,
    pub task_id: String,
    pub scope: String,
    pub items: Vec<Artifact>,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total: u64,
    pub complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactBytes {
    pub version: u32,
    pub task_id: String,
    pub artifact_id: String,
    pub offset: u64,
    pub bytes_read: u64,
    pub next_offset: u64,
    pub size: u64,
    pub sha256: String,
    pub encoding: String,
    pub data_base64: String,
    pub eof: bool,
}
