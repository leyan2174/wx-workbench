//! Shared, path-free selection for one durable history export.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_LIMIT: usize = 500;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Markdown,
    Txt,
    Json,
    Yaml,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Txt => "txt",
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::Txt => "text/plain",
            Self::Json => "application/json",
            Self::Yaml => "application/yaml",
        }
    }
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub chat: String,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub format: Format,
}

impl Request {
    pub fn resolved_window(&self) -> Result<(Option<i64>, Option<i64>)> {
        ensure!(
            !self.chat.trim().is_empty()
                && self.chat.len() <= 256
                && !self.chat.chars().any(char::is_control),
            "Invalid history chat"
        );
        ensure!(
            self.limit > 0 && i64::try_from(self.limit).is_ok(),
            "Invalid history limit"
        );
        for date in [&self.since, &self.until].into_iter().flatten() {
            ensure!(
                !date.is_empty() && date.len() <= 19 && date.is_ascii(),
                "Invalid history date"
            );
        }
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
            "Invalid history time range"
        );
        Ok((since, until))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuerySummary {
    pub username: String,
    pub since_ts: Option<i64>,
    pub until_ts: Option<i64>,
    pub limit: usize,
    pub messages: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryExportResult {
    pub version: u32,
    pub scope: String,
    pub finalized: bool,
    pub outcome: Option<super::task_artifacts::ExportOutcome>,
    pub format: Format,
    pub query: Option<HistoryQuerySummary>,
    pub artifact_count: u64,
    pub artifacts_complete: bool,
    pub diagnostics: Vec<super::task_artifacts::Diagnostic>,
}

impl HistoryExportResult {
    pub(crate) fn empty(format: Format) -> Self {
        Self {
            version: 1,
            scope: "chat_history".into(),
            finalized: false,
            outcome: None,
            format,
            query: None,
            artifact_count: 0,
            artifacts_complete: false,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn validate(&self) -> bool {
        self.version == 1
            && self.scope == "chat_history"
            && self.artifact_count <= 1
            && (!self.finalized || (self.artifact_count == 1 && self.query.is_some()))
            && (self.artifact_count == 0 || (self.finalized && self.query.is_some()))
            && (!self.artifacts_complete || self.finalized)
            && self.query.as_ref().is_none_or(|q| {
                !q.username.is_empty()
                    && q.username.len() <= 256
                    && !q.username.chars().any(char::is_control)
                    && q.limit > 0
                    && i64::try_from(q.limit).is_ok()
                    && q.messages <= q.limit as u64
                    && q.since_ts.zip(q.until_ts).is_none_or(|(a, b)| a <= b)
            })
            && self.diagnostics.len() <= 16
            && self.diagnostics.iter().all(|d| {
                d.count <= super::task_artifacts::MAX_SAFE_INTEGER
                    && matches!(
                        d.code.as_str(),
                        "history_query_warning"
                            | "history_export_failed"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{
        plan,
        protocol::{Kind, Options, Submission},
        settings::Settings,
    };
    use serde_json::json;

    #[test]
    fn defaults_large_limit_and_existing_date_semantics() {
        let mut request: Request = serde_json::from_value(json!({"chat":"peer"})).unwrap();
        assert_eq!(request.limit, 500);
        assert_eq!(request.format, Format::Markdown);
        request.limit = 10001;
        request.since = Some("2026-09-18".into());
        request.until = Some("2026-09-18".into());
        let (since, until) = request.resolved_window().unwrap();
        assert_eq!(
            since,
            Some(crate::service::time::parse_time("2026-09-18 00:00:00").unwrap())
        );
        assert_eq!(
            until,
            Some(crate::service::time::parse_time("2026-09-18 23:59:59").unwrap())
        );
        request.limit = i64::MAX as usize;
        assert!(request.resolved_window().is_ok());
        request.limit += 1;
        assert!(request.resolved_window().is_err());
        request.limit = 0;
        assert!(request.resolved_window().is_err());
    }

    #[test]
    fn invalid_request_and_wrong_kind_are_rejected() {
        for value in [
            json!({}),
            json!({"chat":"peer","format":"csv"}),
            json!({"chat":"peer","output":"x"}),
        ] {
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
        for value in [
            json!({"chat":" "}),
            json!({"chat":"peer","since":"1"}),
            json!({"chat":"peer","since":"2026-09-19","until":"2026-09-18"}),
        ] {
            assert!(serde_json::from_value::<Request>(value)
                .unwrap()
                .resolved_window()
                .is_err());
        }
        let selection = serde_json::from_value(json!({"chat":"peer"})).unwrap();
        let mut submission = Submission {
            kind: Kind::ExportHistory,
            options: Options {
                history_export: Some(selection),
                ..Default::default()
            },
        };
        assert!(plan::validate(&submission, &Settings::default()).is_ok());
        submission.options.include_images = false;
        assert!(plan::validate(&submission, &Settings::default()).is_err());
        submission.options.include_images = true;
        submission.kind = Kind::ExportAll;
        assert!(plan::validate(&submission, &Settings::default()).is_err());
    }
}
