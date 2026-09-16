//! Connect-only task client. Never starts a daemon or retries a submitted call.
use super::{
    protocol::{Call, Envelope, Reply, MAX_REQUEST_BYTES, VERSION},
    transport::{self, DirectoryGuard, CALL_TIMEOUT},
    worker_keys::{
        decode_database_reply, decode_image_reply, DatabaseReadRequest, DatabaseSnapshot,
        ImageReadRequest, ImageSnapshot, MAX_DATABASE_REPLY_BYTES, MAX_IMAGE_REPLY_BYTES,
    },
};
use crate::runtime::RuntimeContext;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
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
use zeroize::{Zeroize, Zeroizing};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub created: u64,
}

pub fn current_process_identity() -> Result<ProcessIdentity> {
    let pid = std::process::id();
    let handle =
        ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? });
    Ok(ProcessIdentity {
        pid,
        created: created(handle.0)?,
    })
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
    connect_named(runtime, &transport::pipe_name(runtime)?).await
}

pub(crate) async fn connect_named(runtime: &RuntimeContext, name: &str) -> Result<NamedPipeClient> {
    connect_named_bound(runtime, name, None).await
}

fn verify_bound_identity(actual: &ProcessIdentity, expected: &ProcessIdentity) -> Result<()> {
    if actual != expected {
        return Err(super::protocol::ServiceError::new(
            "unauthorized",
            "Task daemon process generation mismatch; no request was sent",
        )
        .into());
    }
    Ok(())
}

async fn connect_named_bound(
    runtime: &RuntimeContext,
    name: &str,
    parent: Option<&ProcessIdentity>,
) -> Result<NamedPipeClient> {
    let directory = DirectoryGuard::open(&runtime.directory)?;
    let bytes = transport::read_identity(&directory.path.join("daemon.pid"), 16 * 1024)?;
    let record: PidRecord =
        serde_json::from_slice(&bytes).context("invalid daemon identity record")?;
    ensure!(
        record.runtime_id == runtime.id,
        "daemon identity belongs to another runtime"
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let pipe = loop {
        match ClientOptions::new().open(name) {
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
    if let Some(parent) = parent {
        // pid came from the connected pipe; verify_process checked its live
        // creation time against this record, not just its on-disk metadata.
        verify_bound_identity(
            &ProcessIdentity {
                pid,
                created: record.created,
            },
            parent,
        )?;
    }
    Ok(pipe)
}

/// A timeout is an ambiguous outcome. Callers must retain their idempotency key.
pub(crate) async fn request(runtime: &RuntimeContext, call: Call) -> Result<Value> {
    request_with_timeout(runtime, call, CALL_TIMEOUT).await
}

/// Pins the actual pipe server generation before credentials or material are sent.
pub(crate) async fn request_bound(
    runtime: &RuntimeContext,
    call: Call,
    parent: &ProcessIdentity,
) -> Result<Value> {
    request_inner(runtime, call, CALL_TIMEOUT, Some(parent)).await
}

pub(crate) async fn request_database_keys(
    runtime: &RuntimeContext,
    request: DatabaseReadRequest,
    parent: &ProcessIdentity,
) -> Result<DatabaseSnapshot> {
    tokio::time::timeout(CALL_TIMEOUT, async {
        let directory = DirectoryGuard::open(&runtime.directory)?;
        let mut pipe =
            connect_named_bound(runtime, &transport::pipe_name(runtime)?, Some(parent)).await?;
        let bytes = transport::read_identity(&directory.path.join("service-token.key"), 64)?;
        ensure!(
            bytes.len() == 64 && bytes.iter().all(|byte| byte.is_ascii_hexdigit()),
            "invalid task token file"
        );
        let mut envelope = Envelope {
            version: VERSION,
            runtime_id: runtime.id.clone(),
            token: String::from_utf8(bytes.to_vec())
                .map_err(|_| anyhow::anyhow!("invalid task token encoding"))?,
            request: Call::WorkerDatabaseKeys { request },
        };
        let encoded = transport::encode(&envelope, MAX_REQUEST_BYTES);
        envelope.token.zeroize();
        let encoded = Zeroizing::new(encoded?);
        transport::write_frame(&mut pipe, &encoded).await?;
        let bytes =
            Zeroizing::new(transport::read_frame(&mut pipe, MAX_DATABASE_REPLY_BYTES).await?);
        tokio::io::AsyncWriteExt::write_all(&mut pipe, &[0]).await?;
        decode_database_reply(&bytes, &runtime.id)
    })
    .await
    .map_err(|_| {
        anyhow::Error::new(transport::framing::FrameError::Timeout)
            .context("worker database request timed out")
    })?
}

pub(crate) async fn request_image_material(
    runtime: &RuntimeContext,
    request: ImageReadRequest,
    parent: &ProcessIdentity,
) -> Result<ImageSnapshot> {
    tokio::time::timeout(CALL_TIMEOUT, async {
        let directory = DirectoryGuard::open(&runtime.directory)?;
        let mut pipe =
            connect_named_bound(runtime, &transport::pipe_name(runtime)?, Some(parent)).await?;
        let bytes = transport::read_identity(&directory.path.join("service-token.key"), 64)?;
        ensure!(
            bytes.len() == 64 && bytes.iter().all(|byte| byte.is_ascii_hexdigit()),
            "invalid task token file"
        );
        let mut envelope = Envelope {
            version: VERSION,
            runtime_id: runtime.id.clone(),
            token: String::from_utf8(bytes.to_vec())
                .map_err(|_| anyhow::anyhow!("invalid task token encoding"))?,
            request: Call::WorkerImageMaterial { request },
        };
        let encoded = transport::encode(&envelope, MAX_REQUEST_BYTES);
        envelope.token.zeroize();
        let encoded = Zeroizing::new(encoded?);
        transport::write_frame(&mut pipe, &encoded).await?;
        let bytes = Zeroizing::new(transport::read_frame(&mut pipe, MAX_IMAGE_REPLY_BYTES).await?);
        tokio::io::AsyncWriteExt::write_all(&mut pipe, &[0]).await?;
        decode_image_reply(&bytes, &runtime.id)
    })
    .await
    .map_err(|_| {
        anyhow::Error::new(transport::framing::FrameError::Timeout)
            .context("worker image request timed out")
    })?
}

pub(crate) async fn request_with_timeout(
    runtime: &RuntimeContext,
    call: Call,
    timeout: std::time::Duration,
) -> Result<Value> {
    request_inner(runtime, call, timeout, None).await
}

async fn request_inner(
    runtime: &RuntimeContext,
    call: Call,
    timeout: std::time::Duration,
    parent: Option<&ProcessIdentity>,
) -> Result<Value> {
    ensure!(
        !timeout.is_zero() && timeout <= std::time::Duration::from_secs(90),
        "Invalid service call deadline"
    );
    let max_response_bytes = call.response_limit();
    tokio::time::timeout(timeout, async {
        let directory = DirectoryGuard::open(&runtime.directory)?;
        let mut pipe = match parent {
            Some(parent) => connect_named_bound(runtime, &transport::pipe_name(runtime)?, Some(parent)).await?,
            None => connect(runtime).await?,
        };
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
    }).await.map_err(|_| anyhow::Error::new(transport::framing::FrameError::Timeout).context("task request timed out; outcome may be unknown, do not retry Submit without its original idempotency key"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_identity_rejects_replacement_and_pid_reuse() {
        let parent = ProcessIdentity {
            pid: 41,
            created: 101,
        };
        assert!(verify_bound_identity(&parent, &parent.clone()).is_ok());
        let wire = serde_json::to_vec(&parent).unwrap();
        assert_eq!(
            serde_json::from_slice::<ProcessIdentity>(&wire).unwrap(),
            parent
        );
        for actual in [
            ProcessIdentity {
                pid: 42,
                created: 101,
            },
            ProcessIdentity {
                pid: 41,
                created: 102,
            },
        ] {
            let error = verify_bound_identity(&actual, &parent).unwrap_err();
            let error = error
                .downcast_ref::<super::super::protocol::ServiceError>()
                .unwrap();
            assert_eq!(error.code, "unauthorized");
            assert!(error.message.contains("no request was sent"));
        }
    }

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
