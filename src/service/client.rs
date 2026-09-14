//! Connect-only task client. Never starts a daemon or retries a submitted call.
use super::{
    protocol::{Call, Envelope, Reply, MAX_REQUEST_BYTES, VERSION},
    transport::{self, DirectoryGuard, CALL_TIMEOUT},
};
use crate::runtime::RuntimeContext;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{
    os::windows::{ffi::OsStringExt, io::AsRawHandle},
    path::PathBuf,
};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
            PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};
use zeroize::Zeroize;

pub(crate) fn startup_race(error: &anyhow::Error) -> bool {
    if error
        .downcast_ref::<super::protocol::ServiceError>()
        .is_some()
    {
        return false;
    }
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::WouldBlock
        ) || error.raw_os_error() == Some(231)
    })
}

/// Read-only readiness probe after the caller has ensured daemon startup.
pub(crate) async fn wait_ready(runtime: &RuntimeContext) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let result = tokio::time::timeout_at(deadline, request(runtime, Call::Info {}))
            .await
            .context("Task service startup exceeded three seconds")?;
        match result {
            Ok(info) => return Ok(info),
            Err(error) if startup_race(&error) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep_until(
                    (tokio::time::Instant::now() + std::time::Duration::from_millis(100))
                        .min(deadline),
                )
                .await;
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Deserialize)]
struct PidRecord {
    pid: u32,
    exe: PathBuf,
    created: u64,
    runtime_id: String,
}

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// windows::System::Pipes is not enabled in the shared Cargo manifest.
#[link(name = "kernel32")]
extern "system" {
    fn GetNamedPipeServerProcessId(pipe: *mut std::ffi::c_void, server_pid: *mut u32) -> i32;
}

fn created(handle: HANDLE) -> Result<u64> {
    let mut birth = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(handle, &mut birth, &mut exit, &mut kernel, &mut user)?;
    }
    Ok((u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime))
}

fn verify_process(pid: u32, record: &PidRecord) -> Result<()> {
    ensure!(pid == record.pid, "task pipe server PID mismatch");
    let handle =
        ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? });
    let mut status = 0;
    unsafe {
        GetExitCodeProcess(handle.0, &mut status)?;
    }
    ensure!(
        status == 259 && created(handle.0)? == record.created,
        "task daemon process identity mismatch"
    );
    let mut name = vec![0u16; 32768];
    let mut length = name.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_FORMAT(0),
            PWSTR(name.as_mut_ptr()),
            &mut length,
        )?;
    }
    let actual =
        PathBuf::from(std::ffi::OsString::from_wide(&name[..length as usize])).canonicalize()?;
    let expected = record.exe.canonicalize()?;
    ensure!(
        actual
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.to_string_lossy()),
        "task daemon executable mismatch"
    );
    unsafe {
        GetExitCodeProcess(handle.0, &mut status)?;
    }
    ensure!(
        status == 259,
        "task daemon exited during identity verification"
    );
    Ok(())
}

/// Verifies the actual connected pipe before reading or transmitting credentials.
pub(crate) async fn connect(runtime: &RuntimeContext) -> Result<NamedPipeClient> {
    let directory = DirectoryGuard::open(&runtime.directory)?;
    let bytes = transport::read_identity(&directory.path.join("daemon.pid"), 16 * 1024)?;
    let record: PidRecord =
        serde_json::from_slice(&bytes).context("invalid daemon identity record")?;
    ensure!(
        record.runtime_id == runtime.id,
        "daemon identity belongs to another runtime"
    );
    let name = transport::pipe_name(runtime)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let pipe = loop {
        match ClientOptions::new().open(&name) {
            Ok(pipe) => break pipe,
            Err(error)
                if error.raw_os_error() == Some(231) && tokio::time::Instant::now() < deadline =>
            {
                // No bytes were sent: this only waits for an available pipe instance.
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(error) => {
                return Err(error)
                    .context("existing task daemon unavailable; no daemon was started")
            }
        }
    };
    let mut pid = 0;
    ensure!(
        unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } != 0,
        "cannot verify task pipe server PID"
    );
    verify_process(pid, &record)?;
    Ok(pipe)
}

/// A timeout is an ambiguous outcome. Callers must retain their idempotency key.
pub(crate) async fn request(runtime: &RuntimeContext, call: Call) -> Result<Value> {
    request_with_timeout(runtime, call, CALL_TIMEOUT).await
}

pub(crate) async fn request_with_timeout(
    runtime: &RuntimeContext,
    call: Call,
    timeout: std::time::Duration,
) -> Result<Value> {
    ensure!(
        !timeout.is_zero() && timeout <= std::time::Duration::from_secs(90),
        "Invalid service call deadline"
    );
    let max_response_bytes = call.response_limit();
    tokio::time::timeout(timeout, async {
        let directory = DirectoryGuard::open(&runtime.directory)?;
        let mut pipe = connect(runtime).await?;
        let bytes = transport::read_identity(&directory.path.join("service-token.key"), 64)?;
        ensure!(bytes.len() == 64 && bytes.iter().all(|b| b.is_ascii_hexdigit()), "invalid task token file");
        let mut envelope = Envelope { version: VERSION, runtime_id: runtime.id.clone(), token: String::from_utf8(bytes.to_vec()).map_err(|_| anyhow::anyhow!("invalid task token encoding"))?, request: call };
        let encoded = transport::encode(&envelope, MAX_REQUEST_BYTES);
        envelope.token.zeroize();
        let encoded = zeroize::Zeroizing::new(encoded?);
        transport::write_frame(&mut pipe, &encoded).await?;
        let bytes = transport::read_frame(&mut pipe, max_response_bytes).await?;
        // Let the server close only after its entire response was consumed.
        tokio::io::AsyncWriteExt::write_all(&mut pipe, &[0]).await?;
        let reply: Reply = serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid task reply"))?;
        ensure!(reply.version == VERSION, "unsupported task reply version");
        ensure!(reply.runtime_id == runtime.id, "task reply runtime mismatch");
        if !reply.ok {
            if let Some(error) = reply.error { return Err(error.into()); }
            anyhow::bail!("task service failed without an error");
        }
        ensure!(reply.error.is_none(), "inconsistent task reply");
        Ok(reply.data)
    }).await.context("task request timed out; outcome may be unknown, do not retry Submit without its original idempotency key")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_checks_pid_birth_and_executable() {
        let pid = std::process::id();
        let handle = ProcessHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).unwrap()
        });
        let mut record = PidRecord {
            pid,
            exe: std::env::current_exe().unwrap(),
            created: created(handle.0).unwrap(),
            runtime_id: "test".into(),
        };
        assert!(verify_process(pid, &record).is_ok());
        assert!(verify_process(pid.wrapping_add(1), &record).is_err());
        record.created += 1;
        assert!(verify_process(pid, &record).is_err());
        record.created -= 1;
        record.exe = std::env::temp_dir().join("not-the-daemon.exe");
        assert!(verify_process(pid, &record).is_err());
    }
}
