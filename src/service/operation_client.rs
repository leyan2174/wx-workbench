//! Foreground CLI adapter: submit one typed operation, forward bytes, cancel on disconnect.
use super::{
    client,
    operation_protocol::{reserved_environment, Environment, Invocation, Page},
    operations::Operation,
    protocol::Call,
};
use crate::runtime::RuntimeContext;
use anyhow::{Context, Result};
use std::io::Write;

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

fn run_inner(operation: Operation, capture: bool) -> Result<Vec<u8>> {
    operation.validate_request()?;
    let runtime = RuntimeContext::for_operation()?;
    let environment = Environment(
        std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .filter(|(name, _)| !reserved_environment(name))
            .collect(),
    );
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
    super::query_client::ensure_running_quiet(&runtime)?;
    let cancellation = crate::toolkit::monitor::ConsoleCancellation::install()?;
    let token = cancellation.token();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async {
        let info = client::wait_ready(&runtime).await?;
        anyhow::ensure!(info["operation_api"] == 1, "后台版本不支持统一业务操作；请先停止旧 daemon，再重新执行命令");
        let id = new_id()?;
        client::request(&runtime, Call::OperationStart { id: id.clone(), invocation: Box::new(invocation) }).await?;
        let outcome = async {
            let mut after = 0u64;
            let mut captured = Vec::new();
            loop {
                let response = tokio::select! {
                    value = client::request(&runtime, Call::OperationPoll { id: id.clone(), after }) => value?,
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
        let _ = client::request(&runtime, Call::OperationCancel { id }).await;
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
