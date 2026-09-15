//! Internal business meaning, independent of transport and legacy JSON envelopes.
use serde_json::Value;

/// Query transport diagnostic containing only protocol metadata, never backend data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum QueryLimitExceeded {
    ResponseLimitExceeded {
        operation: String,
        response_limit_bytes: usize,
    },
    QueryReadLimitExceeded {
        operation: String,
    },
}

impl std::fmt::Display for QueryLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResponseLimitExceeded { operation, response_limit_bytes } => write!(
                f, "response_limit_exceeded: {operation} response exceeds {response_limit_bytes} bytes"
            )?,
            Self::QueryReadLimitExceeded { operation } => write!(
                f, "query_read_limit_exceeded: {operation} exceeds the message read budget; narrow --since/--until for deep pages"
            )?,
        }
        f.write_str("; use --offset and a smaller --limit to paginate")
    }
}

impl std::error::Error for QueryLimitExceeded {}

/// Whitelisted diagnostics safe to carry across adapters; no underlying error chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KeyStoreDiagnostic {
    Missing,
    LegacyMigrationRequired,
    Invalid,
    WrongAccount,
    Protection,
    Conflict,
    Busy,
    Io,
}

impl KeyStoreDiagnostic {
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing => "key_store_missing",
            Self::LegacyMigrationRequired => "key_store_migration_required",
            Self::Invalid => "key_store_invalid",
            Self::WrongAccount => "key_store_wrong_account",
            Self::Protection => "key_store_protection",
            Self::Conflict => "key_store_conflict",
            Self::Busy => "key_store_busy",
            Self::Io => "key_store_io",
        }
    }
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "key_store_missing" => Self::Missing,
            "key_store_migration_required" => Self::LegacyMigrationRequired,
            "key_store_invalid" => Self::Invalid,
            "key_store_wrong_account" => Self::WrongAccount,
            "key_store_protection" => Self::Protection,
            "key_store_conflict" => Self::Conflict,
            "key_store_busy" => Self::Busy,
            "key_store_io" => Self::Io,
            _ => return None,
        })
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => {
                "Encrypted key store is missing; explicit key acquisition or migration is required"
            }
            Self::LegacyMigrationRequired => {
                "Legacy key material requires explicit migration; plaintext fallback is disabled"
            }
            Self::Invalid => "Invalid key store format, version or key material",
            Self::WrongAccount => "Key store belongs to a different account",
            Self::Protection => "Current-user DPAPI protection failed; no plaintext fallback",
            Self::Conflict => {
                "Key store update conflicts with the current revision or existing material"
            }
            Self::Busy => "Account key store is being updated; retry after the current update",
            Self::Io => "Key store file or path validation failed",
        }
    }
    pub fn outcome(self) -> BusinessOutcome {
        match self {
            Self::Missing | Self::LegacyMigrationRequired | Self::Conflict | Self::Busy => {
                BusinessOutcome::Refused
            }
            _ => BusinessOutcome::Failure,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessOutcome {
    Success,
    Partial,
    Refused,
    Failure,
}

impl BusinessOutcome {
    /// Compatibility boundary: inspect only documented top-level result markers.
    /// Message bodies, nested tool content, and domain-specific statuses are not guessed.
    pub fn from_legacy(data: &Value) -> Self {
        let invalid = ["ok", "success"]
            .into_iter()
            .any(|key| data.get(key).is_some_and(|value| !value.is_boolean()))
            || data
                .get("exit_code")
                .is_some_and(|value| value.as_i64().is_none());
        if invalid {
            return Self::Failure;
        }
        match data.get("status").and_then(Value::as_str) {
            Some("partial" | "partial_success") => return Self::Partial,
            Some("refused" | "denied" | "ambiguous" | "unsupported" | "not_found") => {
                return Self::Refused
            }
            Some("error" | "failed" | "failure" | "cancelled" | "interrupted") => {
                return Self::Failure
            }
            _ => {}
        }
        if data.get("error").is_some_and(|value| !value.is_null())
            || ["ok", "success"]
                .into_iter()
                .any(|key| data.get(key) == Some(&Value::Bool(false)))
        {
            return Self::Failure;
        }
        match data.get("exit_code").and_then(Value::as_i64) {
            None | Some(0) => Self::Success,
            Some(20) => Self::Partial,
            Some(21) => Self::Refused,
            Some(_) => Self::Failure,
        }
    }

    pub fn from_counts(succeeded: u64, failed: u64) -> Self {
        match (succeeded, failed) {
            (_, 0) => Self::Success,
            (0, _) => Self::Failure,
            _ => Self::Partial,
        }
    }

    /// Reserved worker codes; other nonzero legacy codes remain ordinary failure.
    pub fn worker_exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Partial => 20,
            Self::Refused => 21,
            Self::Failure => 1,
        }
    }

    pub fn from_worker_exit(code: i32) -> Self {
        match code {
            0 => Self::Success,
            20 => Self::Partial,
            21 => Self::Refused,
            _ => Self::Failure,
        }
    }

    pub fn public_message(self) -> &'static str {
        match self {
            Self::Success => "Operation completed",
            Self::Partial => "Operation partially completed; successful artifacts were preserved",
            Self::Refused => "Business request refused",
            Self::Failure => "Business operation failed",
        }
    }

    pub fn service_code(self) -> &'static str {
        match self {
            Self::Success => "business_success",
            Self::Partial => "business_partial",
            Self::Refused => "business_refused",
            Self::Failure => "business_failed",
        }
    }

    pub fn require_success(self) -> Result<(), BusinessFailure> {
        if self == Self::Success {
            Ok(())
        } else {
            Err(BusinessFailure(self, None, None))
        }
    }
}

/// Contains no backend error text, paths, key material or response payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusinessFailure(
    pub BusinessOutcome,
    pub(crate) Option<i32>,
    pub(crate) Option<KeyStoreDiagnostic>,
);

impl BusinessFailure {
    pub fn legacy_exit_code(self) -> Option<i32> {
        self.1
    }
    pub fn diagnostic(self) -> Option<KeyStoreDiagnostic> {
        self.2
    }
    pub fn public_message(self) -> &'static str {
        self.2
            .map_or(self.0.public_message(), KeyStoreDiagnostic::message)
    }
    pub fn service_code(self) -> &'static str {
        self.2
            .map_or(self.0.service_code(), KeyStoreDiagnostic::code)
    }
    pub fn from_service_code(code: &str) -> Option<Self> {
        if let Some(diagnostic) = KeyStoreDiagnostic::from_code(code) {
            return Some(Self(diagnostic.outcome(), None, Some(diagnostic)));
        }
        let outcome = match code {
            "business_partial" => BusinessOutcome::Partial,
            "business_refused" => BusinessOutcome::Refused,
            "business_failed" => BusinessOutcome::Failure,
            _ => return None,
        };
        Some(Self(outcome, None, None))
    }
}

impl std::fmt::Display for BusinessFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.public_message())
    }
}
impl std::error::Error for BusinessFailure {}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod tests;
