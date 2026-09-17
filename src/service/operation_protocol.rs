//! Typed foreground operations. Payloads are transient, never task journal entries.
use super::operations::Operation;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use zeroize::Zeroize;

pub const CHUNK_BYTES: usize = 4096;
pub const BUFFER_BYTES: usize = 512 * 1024;
pub const PAGE_BYTES: usize = 128 * 1024;
pub const LEASE_SECS: u64 = 15;

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Environment(pub BTreeMap<String, String>);

impl Drop for Environment {
    fn drop(&mut self) {
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub operation: Operation,
    pub cwd: PathBuf,
    pub environment: Environment,
}

impl std::fmt::Debug for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub seq: u64,
    pub stderr: bool,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub chunks: Vec<Chunk>,
    pub exit_code: Option<i32>,
}

pub fn reserved_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("WX_DAEMON_") || name == "WX_CLI_EXPECTED_RUNTIME"
}

impl Invocation {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.cwd.is_absolute() && self.cwd.is_dir(),
            "Invalid operation working directory"
        );
        anyhow::ensure!(
            self.environment.0.len() <= 512,
            "Operation environment exceeds limit"
        );
        let mut names = std::collections::HashSet::new();
        let mut bytes = 0usize;
        for (name, value) in &self.environment.0 {
            anyhow::ensure!(
                !name.is_empty()
                    && !name.contains(['=', '\0'])
                    && !value.contains('\0')
                    && !reserved_environment(name)
                    && names.insert(name.to_ascii_uppercase()),
                "Invalid operation environment"
            );
            bytes = bytes.saturating_add(name.len()).saturating_add(value.len());
        }
        anyhow::ensure!(bytes <= 48 * 1024, "Operation environment exceeds limit");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation() -> Invocation {
        Invocation {
            operation: crate::service::operations::Operation::Capabilities { json: true },
            cwd: std::env::temp_dir(),
            environment: Environment(BTreeMap::new()),
        }
    }

    #[test]
    fn context_rejects_internal_flags_case_aliases_and_invalid_entries() {
        for (name, value) in [
            ("WX_DAEMON_MODE", "1"),
            ("wx_daemon_operation_worker", "1"),
            ("WX_CLI_EXPECTED_RUNTIME", "other"),
            ("A=B", "value"),
            ("A", "bad\0value"),
        ] {
            let mut invocation = invocation();
            invocation.environment.0.insert(name.into(), value.into());
            assert!(invocation.validate().is_err());
        }
        let mut invocation = invocation();
        invocation.environment.0.insert("Path".into(), "one".into());
        invocation.environment.0.insert("PATH".into(), "two".into());
        assert!(invocation.validate().is_err());
    }

    #[test]
    fn context_is_bounded_and_debug_never_contains_payloads() {
        let mut invocation = invocation();
        invocation
            .environment
            .0
            .insert("API_CREDENTIAL".into(), "synthetic-private-value".into());
        assert!(invocation.validate().is_ok());
        assert!(!format!("{invocation:?}").contains("synthetic-private-value"));
        invocation
            .environment
            .0
            .insert("TOO_BIG".into(), "x".repeat(48 * 1024));
        assert!(invocation.validate().is_err());
        invocation.environment.0.clear();
        invocation.cwd = "relative".into();
        assert!(invocation.validate().is_err());
    }

    #[test]
    fn wire_context_rejects_unknown_fields() {
        let mut value = serde_json::to_value(invocation()).unwrap();
        value["executable"] = serde_json::json!("cmd.exe");
        assert!(serde_json::from_value::<Invocation>(value).is_err());
    }
}
