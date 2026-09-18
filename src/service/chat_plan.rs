//! Account-scoped, immutable plan references; no caller-selected filesystem paths.
pub use super::operation_requests::plan::Mode;
use super::task_artifacts::{Diagnostic, ExportOutcome};
pub use crate::business::chat_plan::SizeMode;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ROWS: usize = 100_000;
pub const MAX_READ_BYTES: usize = 1024 * 1024;
fn estimate() -> SizeMode {
    SizeMode::Estimate
}
fn one() -> u8 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRef {
    pub task_id: String,
    pub artifact_id: String,
    pub sha256: String,
}
impl PlanRef {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [&self.task_id, &self.artifact_id, &self.sha256]
                .iter()
                .all(|id| super::protocol::valid_task_id(id)),
            "Invalid plan reference"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(default)]
    pub users: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_users: Vec<String>,
    #[serde(default = "estimate")]
    pub size_mode: SizeMode,
    #[serde(default = "one")]
    pub threads: u8,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        ensure!((1..=6).contains(&self.threads), "Plan threads must be 1..6");
        for name in self.users.iter().flatten().chain(self.exclude_users.iter()) {
            ensure!(
                !name.is_empty() && name.len() <= 1024 && !name.chars().any(char::is_control),
                "Invalid plan username"
            );
        }
        self.resolved_window()?;
        Ok(())
    }
    pub fn resolved_window(&self) -> Result<(Option<i64>, Option<i64>)> {
        let start = self
            .start
            .as_deref()
            .map(super::time::parse_timestamp)
            .transpose()?;
        let end = self
            .end
            .as_deref()
            .map(super::time::parse_timestamp)
            .transpose()?;
        ensure!(
            !matches!((start, end), (Some(a), Some(b)) if a > b),
            "Invalid plan time range"
        );
        Ok((start, end))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub username: String,
    pub export: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub plan_ref: PlanRef,
    pub changes: Vec<Change>,
}
impl ReviewRequest {
    pub fn validate(&self) -> Result<()> {
        self.plan_ref.validate()?;
        let mut seen = std::collections::HashSet::new();
        for change in &self.changes {
            ensure!(
                !change.username.is_empty()
                    && change.username.len() <= 1024
                    && !change.username.chars().any(char::is_control)
                    && seen.insert(&change.username),
                "Invalid or duplicate plan username"
            );
            ensure!(
                matches!(change.export.as_str(), "" | "0" | "1"),
                "Invalid plan selection flag"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub plan_ref: PlanRef,
    #[serde(default)]
    pub plan_mode: Mode,
    #[serde(default)]
    pub dry_run: bool,
}
impl ApplyRequest {
    pub fn validate(&self) -> Result<()> {
        self.plan_ref.validate()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub export: String,
    pub index: usize,
    pub username: String,
    pub chat_name: String,
    pub chat_type: String,
    pub message_count: i64,
    pub first_time: String,
    pub last_time: String,
    pub attachment_estimated_bytes: i64,
    pub attachment_scanned_bytes: Option<i64>,
    pub total_estimated_bytes: i64,
    pub size_status: String,
    pub selected: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadReply {
    pub version: u32,
    pub plan_ref: PlanRef,
    pub plan_mode: Mode,
    pub rows: Vec<Row>,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total: u64,
    pub selected_count: u64,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    pub source_kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanResult {
    pub version: u32,
    pub scope: String,
    pub finalized: bool,
    pub outcome: Option<ExportOutcome>,
    pub published_plan_ref: Option<PlanRef>,
    pub parent_ref: Option<PlanRef>,
    pub row_count: u64,
    pub partial_rows: u64,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    pub source_kind: String,
    pub artifact_count: u64,
    pub artifacts_complete: bool,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyResult {
    pub version: u32,
    pub scope: String,
    pub finalized: bool,
    pub outcome: Option<ExportOutcome>,
    pub plan_ref: PlanRef,
    pub plan_mode: Mode,
    pub dry_run: bool,
    pub selected_count: u64,
    pub published_count: u64,
    pub failed_count: u64,
    pub messages: u64,
    pub artifact_count: u64,
    pub artifacts_complete: bool,
    pub diagnostics: Vec<Diagnostic>,
}

fn diagnostics_valid(items: &[Diagnostic]) -> bool {
    items.len() <= 16
        && items.iter().all(|item| {
            item.count <= super::task_artifacts::MAX_SAFE_INTEGER
                && matches!(
                    item.code.as_str(),
                    "plan_partial"
                        | "plan_export_failed"
                        | "chat_export_failed"
                        | "export_interrupted"
                        | "artifact_finalization_timeout"
                        | "artifact_changed"
                        | "artifact_unavailable"
                        | "artifact_unsafe"
                        | "artifact_limit_exceeded"
                        | "result_unavailable"
                )
        })
}
impl PlanResult {
    pub(crate) fn validate(&self) -> bool {
        self.version == 1
            && self.scope == "chat_plan"
            && self.source_kind == "runtime_snapshot"
            && self.row_count <= MAX_ROWS as u64
            && self.partial_rows <= self.row_count
            && self.artifact_count <= 1
            && self.artifact_count == u64::from(self.published_plan_ref.is_some())
            && (!self.finalized || self.published_plan_ref.is_some())
            && (self.published_plan_ref.is_none() || self.finalized)
            && (!self.artifacts_complete || self.finalized)
            && (!self.finalized || self.outcome.is_some())
            && (self.outcome != Some(ExportOutcome::Success)
                || (self.artifacts_complete && self.partial_rows == 0))
            && !matches!((self.start_ts, self.end_ts), (Some(a), Some(b)) if a > b)
            && self
                .published_plan_ref
                .as_ref()
                .is_none_or(|r| r.validate().is_ok())
            && self
                .parent_ref
                .as_ref()
                .is_none_or(|r| r.validate().is_ok())
            && diagnostics_valid(&self.diagnostics)
    }
}
impl ApplyResult {
    pub(crate) fn validate(&self) -> bool {
        self.version == 1
            && self.scope == "chat_plan_apply"
            && self.plan_ref.validate().is_ok()
            && self.selected_count <= MAX_ROWS as u64
            && self.published_count <= self.selected_count
            && self.failed_count <= self.selected_count
            && self.artifact_count == self.published_count
            && self.artifact_count <= super::task_artifacts::MAX_ARTIFACTS as u64
            && self.messages <= super::task_artifacts::MAX_SAFE_INTEGER
            && (!self.dry_run || self.artifact_count == 0)
            && (!self.artifacts_complete || self.finalized)
            && (!self.finalized || self.outcome.is_some())
            && (!self.artifacts_complete
                || self.dry_run
                || self.published_count.saturating_add(self.failed_count) >= self.selected_count)
            && (self.outcome != Some(ExportOutcome::Success)
                || (self.artifacts_complete
                    && self.failed_count == 0
                    && (self.dry_run || self.published_count == self.selected_count)))
            && diagnostics_valid(&self.diagnostics)
    }
}
