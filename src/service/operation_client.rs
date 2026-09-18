//! Foreground CLI adapter: submit one typed operation, forward bytes, cancel on disconnect.
use super::{
    client,
    operation_protocol::{reserved_environment, Environment, Invocation, Page},
    operations::Operation,
    protocol::Call,
};
use crate::runtime::RuntimeContext;
use anyhow::{Context, Result};
use std::{ffi::OsString, io::Write};

#[derive(Debug)]
pub struct OperationExit(pub i32);
impl std::fmt::Display for OperationExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Operation exited with status {}", self.0)
    }
}
impl std::error::Error for OperationExit {}

pub fn run(operation: Operation) -> Result<()> {
    run_inner(operation, false).map(|_| ())
}

pub fn run_capture(operation: Operation) -> Result<Vec<u8>> {
    run_inner(operation, true)
}

/// Foreground invocation bound to the host's already-selected account.
pub fn run_for(runtime: &RuntimeContext, operation: Operation) -> Result<()> {
    run_bound(runtime, operation, false).map(|_| ())
}

fn operation_environment(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Environment {
    Environment(
        vars.into_iter()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            // cmd.exe carries per-drive working directories as pseudo variables such as =C:.
            .filter(|(name, _)| !cfg!(windows) || !name.starts_with('='))
            .filter(|(name, _)| !reserved_environment(name))
            .collect(),
    )
}

fn run_inner(operation: Operation, capture: bool) -> Result<Vec<u8>> {
    operation.validate_request()?;
    let runtime = RuntimeContext::for_operation()?;
    run_bound(&runtime, operation, capture)
}

fn run_bound(runtime: &RuntimeContext, operation: Operation, capture: bool) -> Result<Vec<u8>> {
    operation.validate_request()?;
    let environment = operation_environment(std::env::vars_os());
    let invocation = Invocation {
        operation,
        cwd: std::env::current_dir()?.canonicalize()?,
        environment,
    };
    invocation.validate()?;
    // Reject oversized inputs before daemon startup; nothing is executed or retried here.
    anyhow::ensure!(
        serde_json::to_vec(&invocation)?.len() + 1024 <= super::protocol::MAX_REQUEST_BYTES,
        "Operation request exceeds limit"
    );
    super::query_client::ensure_running_quiet(runtime)?;
    let cancellation = crate::infrastructure::cancellation::ConsoleCancellation::install()?;
    let token = cancellation.token();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async {
        let info = client::wait_ready(runtime).await?;
        anyhow::ensure!(info["operation_api"] == 1, "后台版本不支持统一业务操作；请先停止旧 daemon，再重新执行命令");
        let id = new_id()?;
        client::request(runtime, Call::OperationStart { id: id.clone(), invocation: Box::new(invocation) }).await?;
        let outcome = async {
            let mut after = 0u64;
            let mut captured = Vec::new();
            loop {
                let response = tokio::select! {
                    value = client::request(runtime, Call::OperationPoll { id: id.clone(), after }) => value?,
                    _ = token.cancelled() => return Err(OperationExit(130).into()),
                };
                let page: Page = serde_json::from_value(response).context("Invalid operation response")?;
                for chunk in page.chunks {
                    anyhow::ensure!(chunk.seq == after + 1, "Operation output sequence is incomplete");
                    if chunk.stderr {
                        let mut output = std::io::stderr().lock();
                        output.write_all(&chunk.bytes)?;
                        output.flush()?;
                    } else {
                        if capture {
                            anyhow::ensure!(captured.len() + chunk.bytes.len() <= 1024 * 1024,
                                "Operation captured output exceeds limit");
                            captured.extend_from_slice(&chunk.bytes);
                        }
                        let mut output = std::io::stdout().lock();
                        output.write_all(&chunk.bytes)?;
                        output.flush()?;
                    }
                    after = chunk.seq;
                }
                if let Some(code) = page.exit_code {
                    return if code == 0 { Ok(captured) } else { Err(OperationExit(code).into()) };
                }
            }
        }.await;
        // Terminal entries are released; running entries are cancelled and reaped by the daemon.
        let _ = client::request(runtime, Call::OperationCancel { id }).await;
        outcome
    })
}

fn new_id() -> Result<String> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut bytes = [0u8; 32];
    unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut bytes,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
        .ok()?;
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(entries: &[(&str, &str)]) -> Environment {
        operation_environment(
            entries
                .iter()
                .map(|&(name, value)| (OsString::from(name), OsString::from(value))),
        )
    }

    fn invocation(environment: Environment) -> Invocation {
        Invocation {
            operation: crate::service::operations::Operation::Capabilities { json: true },
            cwd: std::env::temp_dir(),
            environment,
        }
    }

    #[cfg(windows)]
    #[test]
    fn cmd_equivalent_environment_filters_drive_pseudo_variables() {
        let environment = environment(&[
            ("=C:", r"C:\synthetic\work"),
            ("=D:", r"D:\synthetic\other"),
            ("Path", r"C:\Windows\System32"),
            ("ComSpec", r"C:\Windows\System32\cmd.exe"),
            ("WX_TEST_VALUE", "value=with=equals"),
            ("WX_TEST_EMPTY", ""),
            ("wx_daemon_operation_worker", "1"),
            ("WX_CLI_EXPECTED_RUNTIME", "synthetic"),
        ]);
        assert_eq!(environment.0.len(), 4);
        assert_eq!(environment.0["Path"], r"C:\Windows\System32");
        assert_eq!(environment.0["ComSpec"], r"C:\Windows\System32\cmd.exe");
        assert_eq!(environment.0["WX_TEST_VALUE"], "value=with=equals");
        assert_eq!(environment.0["WX_TEST_EMPTY"], "");
        let invocation = invocation(environment);
        invocation.validate().unwrap();
        let wire = serde_json::to_vec(&invocation).unwrap();
        let decoded: Invocation = serde_json::from_slice(&wire).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.environment.0, invocation.environment.0);
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_keeps_leading_equals_for_validation() {
        let environment = environment(&[("=C:", "synthetic")]);
        assert!(environment.0.contains_key("=C:"));
        assert!(invocation(environment).validate().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn cmd_pseudo_variables_do_not_hide_invalid_ordinary_variables() {
        let environment = environment(&[
            ("=C:", r"C:\synthetic\work"),
            ("Path", r"C:\Windows\System32"),
            ("BAD=NAME", "synthetic"),
        ]);
        assert!(!environment.0.contains_key("=C:"));
        assert!(environment.0.contains_key("BAD=NAME"));
        assert!(invocation(environment).validate().is_err());
    }

    #[test]
    fn ordinary_invalid_environment_is_not_silently_filtered() {
        for (name, value) in [
            ("", "value"),
            ("A=B", "value"),
            ("C:", "bad\0value"),
            ("A\0B", "value"),
            ("A", "bad\0value"),
        ] {
            let environment = environment(&[(name, value)]);
            assert_eq!(environment.0.len(), 1);
            assert_eq!(environment.0[name], value);
            assert!(invocation(environment).validate().is_err());
        }
        let environment = environment(&[("Path", "one"), ("PATH", "two")]);
        assert_eq!(environment.0.len(), 2);
        assert!(invocation(environment).validate().is_err());
    }

    #[test]
    fn collected_environment_still_obeys_count_and_byte_limits() {
        let entries = |count| {
            (0..count).map(|i| (OsString::from(format!("VAR_{i}")), OsString::from("value")))
        };
        assert!(invocation(operation_environment(entries(512)))
            .validate()
            .is_ok());
        assert!(invocation(operation_environment(entries(513)))
            .validate()
            .is_err());
        let at_limit = "x".repeat(48 * 1024 - 1);
        assert!(invocation(environment(&[("A", &at_limit)]))
            .validate()
            .is_ok());
        let over_limit = "x".repeat(48 * 1024);
        assert!(invocation(environment(&[("A", &over_limit)]))
            .validate()
            .is_err());
    }

    #[test]
    fn raw_invocation_still_rejects_pseudo_and_reserved_variables() {
        for name in [
            "=C:",
            "=D:",
            "WX_DAEMON_MODE",
            "wx_daemon_operation_worker",
            "WX_CLI_EXPECTED_RUNTIME",
            "wx_cli_expected_runtime",
        ] {
            let raw = Environment([(name.into(), "synthetic".into())].into_iter().collect());
            assert!(invocation(raw).validate().is_err());
        }
    }
}
