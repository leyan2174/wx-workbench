//! Path-free raw SILK export selection and durable, complete-group results.
use super::task_artifacts::{Diagnostic, ExportOutcome, MAX_ARTIFACTS, MAX_SAFE_INTEGER};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    pub chat: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
    pub offset: usize,
    pub overwrite: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selection_preserves_none_zero_and_identity_without_new_limit() {
        let default: Request = serde_json::from_value(json!({})).unwrap();
        assert_eq!(default.limit, None);
        assert_eq!(default.offset, 0);
        let zero: Request = serde_json::from_value(json!({"limit":0,"chat":" peer "})).unwrap();
        assert_eq!(zero.limit, Some(0));
        assert_eq!(zero.chat.as_deref(), Some(" peer "));
        assert!(zero.resolved_window().is_ok());
        assert!(Request {
            limit: Some(usize::MAX),
            ..Default::default()
        }
        .resolved_window()
        .is_ok());
        for invalid in [
            json!({"chat":" \t"}),
            json!({"overwrite":true}),
            json!({"since":"2025-02-02","until":"2025-02-01"}),
        ] {
            assert!(serde_json::from_value::<Request>(invalid)
                .unwrap()
                .resolved_window()
                .is_err());
        }
        for unknown in [
            json!({"output":"elsewhere"}),
            json!({"asr":true}),
            json!({"allow_media_write":true}),
        ] {
            assert!(serde_json::from_value::<Request>(unknown).is_err());
        }
    }

    #[test]
    fn date_until_uses_existing_inclusive_end() {
        let request = Request {
            since: Some("2025-02-01".into()),
            until: Some("2025-02-01".into()),
            ..Default::default()
        };
        let (since, until) = request.resolved_window().unwrap();
        assert_eq!(until.unwrap() - since.unwrap(), 86_399);
    }

    #[test]
    fn result_rejects_invented_success_and_incomplete_groups() {
        let mut result = VoiceExportResult::empty(Request::default()).unwrap();
        assert!(result.validate());
        result.outcome = Some(ExportOutcome::Success);
        assert!(!result.validate());
        result.outcome = None;
        result.selected_rows = Some(1);
        result.exported = 1;
        result.associated = 1;
        result.artifact_count = 1;
        assert!(!result.validate());
        result.artifact_count = 2;
        assert!(result.validate());
        result.finalized = true;
        assert!(!result.validate());
        result.artifact_count = 3;
        result.outcome = Some(ExportOutcome::Success);
        result.artifacts_complete = true;
        assert!(result.validate());
        result.scope = "chat_directory".into();
        assert!(!result.validate());
    }
}

impl Request {
    pub fn resolved_window(&self) -> Result<(Option<i64>, Option<i64>)> {
        ensure!(
            self.chat
                .as_ref()
                .is_none_or(|chat| !chat.trim().is_empty()),
            "Empty voice chat"
        );
        ensure!(!self.overwrite, "Voice task overwrite is not supported");
        let since = self
            .since
            .as_deref()
            .map(super::time::parse_time)
            .transpose()?;
        let until = self
            .until
            .as_deref()
            .map(super::time::parse_time_end)
            .transpose()?;
        ensure!(
            since.zip(until).is_none_or(|(a, b)| a <= b),
            "Invalid voice time range"
        );
        Ok((since, until))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionSummary {
    pub request: Request,
    pub since_ts: Option<i64>,
    pub until_ts: Option<i64>,
    pub target_username: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceExportResult {
    pub version: u32,
    pub scope: String,
    pub finalized: bool,
    pub outcome: Option<ExportOutcome>,
    pub selection: SelectionSummary,
    pub selected_rows: Option<u64>,
    pub exported: u64,
    pub associated: u64,
    pub unproven: u64,
    pub incomplete_items: u64,
    pub artifact_count: u64,
    pub artifacts_complete: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl VoiceExportResult {
    pub(crate) fn empty(request: Request) -> Result<Self> {
        let (since_ts, until_ts) = request.resolved_window()?;
        Ok(Self {
            version: 1,
            scope: "raw_voices".into(),
            finalized: false,
            outcome: None,
            selection: SelectionSummary {
                request,
                since_ts,
                until_ts,
                target_username: None,
            },
            selected_rows: None,
            exported: 0,
            associated: 0,
            unproven: 0,
            incomplete_items: 0,
            artifact_count: 0,
            artifacts_complete: false,
            diagnostics: Vec::new(),
        })
    }
    pub(crate) fn validate(&self) -> bool {
        self.version == 1
            && self.scope == "raw_voices"
            && self
                .selection
                .request
                .resolved_window()
                .is_ok_and(|window| window == (self.selection.since_ts, self.selection.until_ts))
            && self.exported <= MAX_ARTIFACTS as u64 / 2
            && self.associated.checked_add(self.unproven) == Some(self.exported)
            && self.incomplete_items <= MAX_SAFE_INTEGER
            && self.incomplete_items >= self.unproven
            && self
                .selected_rows
                .is_none_or(|count| count <= MAX_SAFE_INTEGER && self.exported <= count)
            && (self.exported == 0 || self.selected_rows.is_some())
            && self.artifact_count <= MAX_ARTIFACTS as u64
            && (self.artifact_count == self.exported * 2
                || self.artifact_count == self.exported * 2 + 1)
            && (!self.finalized
                || (self.selected_rows.is_some()
                    && self.artifact_count == self.exported * 2 + 1
                    && self.outcome.is_some()))
            && (!self.artifacts_complete || self.finalized)
            && (self.outcome != Some(ExportOutcome::Success)
                || (self.finalized
                    && self.artifacts_complete
                    && self.incomplete_items == 0
                    && self.selected_rows == Some(self.exported)))
            && self.diagnostics.len() <= 16
            && self.diagnostics.iter().all(|d| {
                d.count <= MAX_SAFE_INTEGER
                    && matches!(
                        d.code.as_str(),
                        "voice_export_failed"
                            | "voice_sources_incomplete"
                            | "voice_unproven"
                            | "voice_item_failed"
                            | "voice_budget_exceeded"
                            | "result_unavailable"
                            | "artifact_limit_exceeded"
                            | "artifact_unsafe"
                            | "artifact_changed"
                            | "artifact_unavailable"
                            | "export_interrupted"
                            | "artifact_finalization_timeout"
                    )
            })
    }
}
