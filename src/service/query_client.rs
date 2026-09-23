//! 共享命名管道客户端与后台生命周期；每次操作固定一个账号运行上下文。
use crate::ipc::{Request, Response};
use crate::runtime::RuntimeContext;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PidFile {
    pid: u32,
    exe: PathBuf,
    created: u64,
    runtime_id: String,
}

pub fn is_alive() -> bool {
    RuntimeContext::load()
        .ok()
        .is_some_and(|runtime| ping(&runtime).unwrap_or(false))
}

pub(crate) fn ensure_running(runtime: &RuntimeContext) -> Result<()> {
    ensure_running_with_notice(runtime, true)
}

pub(crate) fn ensure_running_quiet(runtime: &RuntimeContext) -> Result<()> {
    ensure_running_with_notice(runtime, false)
}

fn ensure_running_with_notice(runtime: &RuntimeContext, notice: bool) -> Result<()> {
    ensure_running_until(runtime, notice, Instant::now() + STARTUP_TIMEOUT)
}

fn ensure_running_until(runtime: &RuntimeContext, notice: bool, deadline: Instant) -> Result<()> {
    if ping_until(runtime, deadline)? {
        return Ok(());
    }
    // 并发客户端串行确认并启动；持锁期间第二个客户端不能再次启动同一后台。
    let _lock = loop {
        match runtime.lock("startup.lock") {
            Ok(lock) => break lock,
            Err(error) => {
                if ping_until(runtime, deadline)? {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    if ping_until(runtime, deadline)? {
        return Ok(());
    }
    ensure!(
        !recorded_process_alive(runtime)?,
        "当前账号后台仍存活但未响应，未覆盖其身份记录；请先执行 daemon stop 或检查日志"
    );
    ensure!(Instant::now() < deadline, "后台启动超时");
    start_daemon(runtime, notice, deadline)
}

fn recorded_process_alive(runtime: &RuntimeContext) -> Result<bool> {
    let bytes = match super::transport::read_identity(
        &runtime.pid_path(),
        super::transport::PROCESS_IDENTITY_LIMIT,
    ) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    let record: PidFile =
        serde_json::from_slice(&bytes).context("后台身份记录损坏，未覆盖原记录")?;
    ensure!(
        record.runtime_id == runtime.id,
        "后台身份记录不属于当前账号"
    );
    match process_handle(record.pid, false) {
        Ok(handle) => active_process_matches(handle.0, &record),
        Err(error) if missing_process(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn missing_process(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|error| {
            error.code()
                == windows::core::HRESULT::from_win32(
                    windows::Win32::Foundation::ERROR_INVALID_PARAMETER.0,
                )
        })
}

fn process_active(handle: windows::Win32::Foundation::HANDLE) -> Result<bool> {
    let mut code = 0;
    unsafe {
        windows::Win32::System::Threading::GetExitCodeProcess(handle, &mut code)?;
    }
    Ok(code == 259) // Windows STILL_ACTIVE；本程序不使用该退出码。
}

fn start_daemon(runtime: &RuntimeContext, notice: bool, deadline: Instant) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?.canonicalize()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(runtime.log_path())?;
    if notice {
        eprintln!("启动当前账号 wx-daemon...");
    }
    let mut child = Command::new(&exe)
        .env("WX_DAEMON_MODE", "1")
        .env(
            "WX_DAEMON_BOOTSTRAP",
            if runtime.is_bootstrap() { "1" } else { "0" },
        )
        .env_remove("WX_DAEMON_OPERATION_WORKER")
        .env_remove("WX_DAEMON_TASK_WORKER")
        .env("WX_CLI_CONFIG", &runtime.config_path)
        .env("WX_CLI_HOME", &runtime.root)
        .env("WX_CLI_EXPECTED_RUNTIME", &runtime.id)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(0x08000008) // 隐藏窗口并脱离控制台。
        .spawn()
        .context("无法启动后台进程")?;
    let result = (|| -> Result<()> {
        let handle = process_handle(child.id(), false)?;
        let record = PidFile {
            pid: child.id(),
            exe,
            created: process_created(handle.0)?,
            runtime_id: runtime.id.clone(),
        };
        std::fs::write(runtime.pid_path(), serde_json::to_vec(&record)?)?;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                bail!(
                    "后台提前退出：{status}，日志：{}",
                    runtime.log_path().display()
                );
            }
            if ping_until(runtime, deadline)? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("后台启动超时，日志：{}", runtime.log_path().display())
    })();
    if result.is_err() {
        // 只终止本次创建且仍持有句柄的子进程，不凭一个可能复用的 PID 清理。
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(runtime.pid_path());
    }
    result
}

pub fn stop_daemon() -> Result<()> {
    stop_runtime(&RuntimeContext::load()?)
}

pub(crate) fn stop_runtime(runtime: &RuntimeContext) -> Result<()> {
    let _lock = runtime.lock("startup.lock")?;
    let path = runtime.pid_path();
    let text =
        match super::transport::read_identity(&path, super::transport::PROCESS_IDENTITY_LIMIT) {
            Ok(text) => text,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                ensure!(
                    !ping(runtime).unwrap_or(false),
                    "后台正在运行但缺少身份记录，拒绝盲目停止"
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        };
    let record: PidFile =
        serde_json::from_slice(&text).context("后台身份记录损坏，拒绝盲目停止")?;
    ensure!(
        record.runtime_id == runtime.id,
        "后台身份记录不属于当前账号"
    );
    match process_handle(record.pid, true) {
        Ok(handle) => {
            if active_process_matches(handle.0, &record)? {
                use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
                // 先请求 daemon 正常停机，留出任务取消和子进程回收时间；失败或超时后再强制终止。
                let graceful = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .and_then(|rt| {
                        rt.block_on(crate::service::client::request(
                            runtime,
                            crate::service::protocol::Call::Shutdown {},
                        ))
                        .map(|_| ())
                        .map_err(std::io::Error::other)
                    });
                unsafe {
                    if graceful.is_err() || WaitForSingleObject(handle.0, 15000).0 != 0 {
                        TerminateProcess(handle.0, 0)?;
                    }
                    ensure!(
                        WaitForSingleObject(handle.0, 5000).0 == 0,
                        "等待后台退出超时"
                    );
                }
            } else {
                // An authenticated ping also rejects a tampered identity record; its
                // failure is not evidence that the recorded process has exited.
                ensure!(!process_active(handle.0)?, "PID 已复用或身份不符，拒绝停止");
            }
        }
        Err(e) => {
            // 仅在操作系统明确报告 PID 无效时当作陈旧记录；权限不足不能视为进程已退出。
            if !missing_process(&e) {
                return Err(e);
            }
        }
    }
    std::fs::remove_file(path)?;
    Ok(())
}

struct ProcessHandle(windows::Win32::Foundation::HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

fn process_handle(pid: u32, terminate: bool) -> Result<ProcessHandle> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    let mut rights = PROCESS_QUERY_LIMITED_INFORMATION;
    if terminate {
        rights |= PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
    }
    Ok(ProcessHandle(unsafe { OpenProcess(rights, false, pid)? }))
}

fn process_created(handle: windows::Win32::Foundation::HANDLE) -> Result<u64> {
    use windows::Win32::Foundation::FILETIME;
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        windows::Win32::System::Threading::GetProcessTimes(
            handle,
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        )?;
    }
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

fn process_matches(handle: windows::Win32::Foundation::HANDLE, record: &PidFile) -> Result<bool> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_FORMAT};
    if process_created(handle)? != record.created {
        return Ok(false);
    }
    let mut name = vec![0u16; 32768];
    let mut len = name.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(name.as_mut_ptr()),
            &mut len,
        )?;
    }
    let actual = PathBuf::from(String::from_utf16_lossy(&name[..len as usize]));
    Ok(normalized_exe(&actual) == normalized_exe(&record.exe))
}

fn active_process_matches(
    handle: windows::Win32::Foundation::HANDLE,
    record: &PidFile,
) -> Result<bool> {
    // 已退出进程的内核对象仍可能被其他句柄保留，查询映像路径不一定可用。
    if !process_active(handle)? {
        return Ok(false);
    }
    match process_matches(handle, record) {
        Ok(matches) => Ok(matches && process_active(handle)?),
        Err(error) => {
            // 身份查询期间也可能退出；仅凭同一句柄的退出状态解除陈旧记录。
            if process_active(handle)? {
                Err(error)
            } else {
                Ok(false)
            }
        }
    }
}

fn normalized_exe(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.into())
        .to_string_lossy()
        .to_lowercase()
}

fn ping(runtime: &RuntimeContext) -> Result<bool> {
    // 健康检查只接受小型 Pong，不能绕过 MCP 等调用方的查询响应限额。
    let response =
        request_with_options(runtime, Request::Ping, Duration::from_secs(1), Some(1024))?;
    Ok(response.data.get("pong").and_then(|v| v.as_bool()) == Some(true))
}

fn ping_until(runtime: &RuntimeContext, deadline: Instant) -> Result<bool> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(super::transport::framing::FrameError::Timeout.into());
    }
    Ok(request_with_options(
        runtime,
        Request::Ping,
        remaining.min(Duration::from_secs(1)),
        Some(1024),
    )
    .map(|response| response.data.get("pong").and_then(|v| v.as_bool()) == Some(true))
    .unwrap_or(false))
}

pub fn send(req: Request) -> Result<Response> {
    let runtime = RuntimeContext::load()?;
    send_for(&runtime, req)
}

pub fn send_with_startup_notice(req: Request, notice: bool) -> Result<Response> {
    let runtime = RuntimeContext::load()?;
    request(&runtime, req, notice)
}

/// 同一批次固定账号身份；配置切换不能将后续请求发往另一条账号管道。
pub(crate) fn send_for(runtime: &RuntimeContext, req: Request) -> Result<Response> {
    request(runtime, req, true)
}

/// MCP 使用已经固定的账号上下文，并在分配完整响应前执行大小限制。
pub(crate) fn send_with_limits(
    runtime: &RuntimeContext,
    req: Request,
    timeout: Duration,
    max_response_bytes: usize,
) -> Result<Response> {
    send_with_limits_and_notice(runtime, req, timeout, max_response_bytes, true)
}

fn send_with_limits_and_notice(
    runtime: &RuntimeContext,
    req: Request,
    timeout: Duration,
    max_response_bytes: usize,
    notice: bool,
) -> Result<Response> {
    super::transport::framing::budget(max_response_bytes)?;
    ensure!(
        !timeout.is_zero() && timeout <= Duration::from_secs(3600),
        "invalid query deadline"
    );
    let started = Instant::now();
    ensure_running_until(runtime, notice, started + timeout.min(STARTUP_TIMEOUT))?;
    let remaining = timeout.saturating_sub(started.elapsed());
    ensure!(!remaining.is_zero(), "后台启动后请求已超时");
    request_with_options(runtime, req, remaining, Some(max_response_bytes))
}

fn request(runtime: &RuntimeContext, req: Request, notice: bool) -> Result<Response> {
    let seconds: u64 = std::env::var("WX_CLI_REQUEST_TIMEOUT_SECS")
        .map(|value| {
            value
                .parse()
                .context("WX_CLI_REQUEST_TIMEOUT_SECS 必须是正整数")
        })
        .unwrap_or(Ok(300))?;
    ensure!(seconds > 0, "WX_CLI_REQUEST_TIMEOUT_SECS 必须大于零");
    let limit = crate::ipc::query_response_limit(&req);
    send_with_limits_and_notice(runtime, req, Duration::from_secs(seconds), limit, notice)
}

#[cfg(test)]
fn request_with_timeout(
    runtime: &RuntimeContext,
    req: Request,
    timeout: Duration,
) -> Result<Response> {
    request_with_options(runtime, req, timeout, None)
}

fn request_with_options(
    runtime: &RuntimeContext,
    req: Request,
    timeout: Duration,
    max_response_bytes: Option<usize>,
) -> Result<Response> {
    use super::transport::framing::{self, FrameError};
    let max_response_bytes =
        max_response_bytes.unwrap_or_else(|| crate::ipc::query_response_limit(&req));
    framing::budget(max_response_bytes)?;
    let effective_limit = max_response_bytes.min(crate::ipc::query_response_limit(&req));
    let operation = req.operation_name().to_owned();
    ensure!(
        !timeout.is_zero() && timeout <= Duration::from_secs(3600),
        "invalid query deadline"
    );
    // 异步 I/O 可在连接、写入和读取任一阶段取消，不留下占用管道的工作线程。
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let result = tokio::time::timeout(timeout, async {
                let mut reader = connect_query(runtime).await?;
                write_query(&mut reader, runtime, req, max_response_bytes).await?;
                let response = async {
                    let bytes = framing::line(&mut reader, max_response_bytes).await?;
                    decode_query_response(&bytes, runtime)
                }
                .await
                .map_err(|error| {
                    if matches!(
                        error.downcast_ref::<FrameError>(),
                        Some(FrameError::Oversize)
                    ) {
                        crate::ipc::outcome::QueryLimitExceeded::ResponseLimitExceeded {
                            operation: operation.clone(),
                            response_limit_bytes: effective_limit,
                        }
                        .into()
                    } else {
                        error
                    }
                })?;
                if !response.ok
                    && response
                        .data
                        .get("error_code")
                        .and_then(serde_json::Value::as_str)
                        == Some("query_read_limit_exceeded")
                {
                    return Err(
                        crate::ipc::outcome::QueryLimitExceeded::QueryReadLimitExceeded {
                            operation: operation.clone(),
                        }
                        .into(),
                    );
                }
                response.require_success()?;
                Ok::<_, anyhow::Error>(response)
            })
            .await;
            result.map_err(|_| anyhow::Error::new(FrameError::Timeout).context("后台请求超时"))?
        })
}

/// Connect-only: verify the OS peer, then its protocol/runtime before business data.
pub(crate) async fn connect_query(
    runtime: &RuntimeContext,
) -> Result<tokio::io::BufReader<tokio::net::windows::named_pipe::NamedPipeClient>> {
    use super::transport::framing::{self, FrameError};
    let name = format!(r"\\.\pipe\{}", runtime.pipe_name());
    let stream = super::client::connect_named(runtime, &name).await?;
    let mut reader = tokio::io::BufReader::new(stream);
    let bytes = tokio::time::timeout(Duration::from_secs(3), framing::line(&mut reader, 1024))
        .await
        .map_err(|_| FrameError::Timeout)
        .context("query v3 handshake required; restart an older daemon explicitly")??;
    let hello: crate::ipc::QueryHello =
        serde_json::from_slice(&bytes).map_err(|_| FrameError::Protocol)?;
    if hello.version != crate::ipc::QUERY_VERSION || hello.runtime_id != runtime.id {
        return Err(FrameError::Protocol.into());
    }
    Ok(reader)
}

pub(crate) async fn write_query(
    reader: &mut tokio::io::BufReader<tokio::net::windows::named_pipe::NamedPipeClient>,
    runtime: &RuntimeContext,
    request: Request,
    response_limit: usize,
) -> Result<()> {
    use super::transport::{self, framing};
    framing::budget(response_limit)?;
    let envelope = crate::ipc::QueryEnvelope {
        version: crate::ipc::QUERY_VERSION,
        runtime_id: runtime.id.clone(),
        response_limit,
        request,
    };
    let bytes = transport::encode(&envelope, crate::ipc::QUERY_REQUEST_LIMIT - 1)?;
    framing::write_line(reader.get_mut(), &bytes, crate::ipc::QUERY_REQUEST_LIMIT).await?;
    Ok(())
}

pub(crate) fn decode_query_response(bytes: &[u8], runtime: &RuntimeContext) -> Result<Response> {
    use super::transport::framing::FrameError;
    use crate::ipc::{QueryReply, QUERY_VERSION};
    let reply: QueryReply = serde_json::from_slice(bytes).map_err(|_| FrameError::Protocol)?;
    let (version, runtime_id) = match &reply {
        QueryReply::Response {
            version,
            runtime_id,
            ..
        }
        | QueryReply::Oversize {
            version,
            runtime_id,
        } => (*version, runtime_id),
    };
    if version != QUERY_VERSION || runtime_id != &runtime.id {
        return Err(FrameError::Protocol.into());
    }
    match reply {
        QueryReply::Response { response, .. } => Ok(response),
        QueryReply::Oversize { .. } => Err(FrameError::Oversize.into()),
    }
}

#[cfg(test)]
async fn read_response<R: tokio::io::AsyncBufRead + Unpin>(
    mut reader: R,
    max_response_bytes: Option<usize>,
) -> Result<Response> {
    let line = super::transport::framing::line(
        &mut reader,
        max_response_bytes.unwrap_or(crate::ipc::QUERY_RESPONSE_LIMIT),
    )
    .await
    .context("后台响应帧不完整或超过大小限制")?;
    let response: Response = serde_json::from_slice(&line).context("解析后台响应失败")?;
    response.require_success()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_identity_limit_rejects_oversize_without_mutating_record() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = RuntimeContext {
            config: crate::config::Config {
                key_store: None,
                db_dir: PathBuf::new(),
                keys_file: PathBuf::new(),
                decrypted_dir: PathBuf::new(),
                wechat_process: String::new(),
            },
            config_path: PathBuf::new(),
            root: PathBuf::new(),
            directory: temp.path().to_path_buf(),
            id: "synthetic-identity-limit".into(),
        };
        assert!(!recorded_process_alive(&runtime).unwrap());
        let handle = process_handle(std::process::id(), false).unwrap();
        let mut bytes = serde_json::to_vec(&PidFile {
            pid: std::process::id(),
            exe: std::env::current_exe().unwrap(),
            created: process_created(handle.0).unwrap(),
            runtime_id: runtime.id.clone(),
        })
        .unwrap();
        bytes.resize(super::super::transport::PROCESS_IDENTITY_LIMIT, b' ');
        std::fs::write(runtime.pid_path(), &bytes).unwrap();
        assert!(recorded_process_alive(&runtime).unwrap());
        bytes.push(b' ');
        // A regressed size check must fail parsing, never reach process termination.
        bytes[0] = b'!';
        std::fs::write(runtime.pid_path(), &bytes).unwrap();
        let before = std::fs::metadata(runtime.pid_path()).unwrap();
        for result in [
            recorded_process_alive(&runtime).map(|_| ()),
            stop_runtime(&runtime),
        ] {
            assert!(format!("{:#}", result.unwrap_err()).contains("exceeds limit"));
        }
        assert_eq!(std::fs::read(runtime.pid_path()).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(runtime.pid_path())
                .unwrap()
                .modified()
                .unwrap(),
            before.modified().unwrap()
        );
        assert!(process_active(handle.0).unwrap());
    }

    #[tokio::test]
    async fn bounded_response_checks_limit_before_json_parse() {
        let reply = b"{\"ok\":true,\"pong\":true}\n";
        assert_eq!(
            read_response(&reply[..], Some(reply.len()))
                .await
                .unwrap()
                .data["pong"],
            true
        );
        assert!(read_response(&reply[..], Some(reply.len() - 1))
            .await
            .unwrap_err()
            .to_string()
            .contains("大小限制"));
        let oversized = vec![b'x'; 1025];
        assert!(read_response(&oversized[..], Some(1024))
            .await
            .unwrap_err()
            .to_string()
            .contains("大小限制"));
        assert!(read_response(&b"not-json\n"[..], Some(1024))
            .await
            .unwrap_err()
            .to_string()
            .contains("解析"));
        assert_eq!(
            read_response(&reply[..], None).await.unwrap().data["pong"],
            true
        );
    }

    #[test]
    fn process_birth_time_prevents_pid_reuse_confusion() {
        let handle = process_handle(std::process::id(), false).unwrap();
        let mut record = PidFile {
            pid: std::process::id(),
            exe: std::env::current_exe().unwrap(),
            created: process_created(handle.0).unwrap(),
            runtime_id: "test".into(),
        };
        assert!(process_matches(handle.0, &record).unwrap());
        record.created += 1;
        assert!(!process_matches(handle.0, &record).unwrap());
    }

    #[test]
    fn connected_but_silent_server_times_out_without_leaking_reader() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = RuntimeContext {
            config: crate::config::Config {
                key_store: None,
                db_dir: PathBuf::new(),
                keys_file: PathBuf::new(),
                decrypted_dir: PathBuf::new(),
                wechat_process: String::new(),
            },
            config_path: PathBuf::new(),
            root: PathBuf::new(),
            directory: temp.path().to_path_buf(),
            id: format!(
                "timeout-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ),
        };
        let handle = process_handle(std::process::id(), false).unwrap();
        std::fs::write(
            runtime.pid_path(),
            serde_json::to_vec(&PidFile {
                pid: std::process::id(),
                exe: std::env::current_exe().unwrap(),
                created: process_created(handle.0).unwrap(),
                runtime_id: runtime.id.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let pipe = runtime.pipe_name();
        let (ready, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            use interprocess::local_socket::{
                tokio::prelude::*, GenericNamespaced, ListenerOptions,
            };
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let listener = ListenerOptions::new()
                        .name(pipe.to_ns_name::<GenericNamespaced>().unwrap())
                        .create_tokio()
                        .unwrap();
                    ready.send(()).unwrap();
                    if let Ok(Ok(mut stream)) =
                        tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
                    {
                        // 接受连接但不返回消息，模拟后台卡在查询中的情况。
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        use tokio::io::AsyncReadExt;
                        let mut byte = [0u8; 1];
                        let count =
                            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte))
                                .await
                                .expect("timed-out client must close its pipe")
                                .expect("read client disconnect");
                        assert_eq!(count, 0, "rejected handshake must not send business data");
                    } else {
                        panic!("synthetic client did not connect");
                    }
                });
        });
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        let result = request_with_timeout(&runtime, Request::Ping, Duration::from_millis(50));
        let elapsed = start.elapsed();
        server.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("超时"));
        assert!(elapsed < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn real_query_pipe_rejects_peer_protocol_and_incomplete_frames() {
        use super::super::transport::framing::FrameError;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::windows::named_pipe::ServerOptions,
        };
        for mode in [
            "pid",
            "birth",
            "exe",
            "version",
            "runtime",
            "oversize",
            "truncated",
            "ok",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let runtime = RuntimeContext {
                config: crate::config::Config {
                    key_store: None,
                    db_dir: PathBuf::new(),
                    keys_file: PathBuf::new(),
                    decrypted_dir: PathBuf::new(),
                    wechat_process: String::new(),
                },
                config_path: PathBuf::new(),
                root: temp.path().into(),
                directory: temp.path().into(),
                id: format!(
                    "query-fixture-{}-{mode}-{}",
                    std::process::id(),
                    chrono::Utc::now().timestamp_nanos_opt().unwrap()
                ),
            };
            let handle = process_handle(std::process::id(), false).unwrap();
            let mut record = PidFile {
                pid: std::process::id(),
                exe: std::env::current_exe().unwrap(),
                created: process_created(handle.0).unwrap(),
                runtime_id: runtime.id.clone(),
            };
            match mode {
                "pid" => record.pid = record.pid.wrapping_add(1),
                "birth" => record.created += 1,
                "exe" => record.exe = temp.path().join("wrong.exe"),
                _ => (),
            }
            std::fs::write(runtime.pid_path(), serde_json::to_vec(&record).unwrap()).unwrap();
            let name = format!(r"\\.\pipe\{}", runtime.pipe_name());
            let mut server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .unwrap();
            let id = runtime.id.clone();
            let reject = matches!(mode, "pid" | "birth" | "exe" | "version" | "runtime");
            let serving = tokio::spawn(async move {
                server.connect().await.unwrap();
                if !matches!(mode, "pid" | "birth" | "exe") {
                    let hello = crate::ipc::QueryHello {
                        version: if mode == "version" {
                            0
                        } else {
                            crate::ipc::QUERY_VERSION
                        },
                        runtime_id: if mode == "runtime" {
                            "other".into()
                        } else {
                            id.clone()
                        },
                    };
                    let _ = server
                        .write_all((serde_json::to_string(&hello).unwrap() + "\n").as_bytes())
                        .await;
                }
                let mut request = [0; 1024];
                let count = server.read(&mut request).await.unwrap_or(0);
                if reject {
                    assert_eq!(count, 0, "business data sent to rejected peer");
                    return;
                }
                assert!(count > 0);
                match mode {
                    "oversize" => server.write_all(&[b'x'; 1025]).await.unwrap(),
                    "truncated" => {
                        server.write_all(b"{\"ok\":true}").await.unwrap();
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    _ => {
                        let reply = crate::ipc::QueryReply::Response {
                            version: crate::ipc::QUERY_VERSION,
                            runtime_id: id,
                            response: Response::ok(serde_json::json!({"pong":true})),
                        };
                        server
                            .write_all((serde_json::to_string(&reply).unwrap() + "\n").as_bytes())
                            .await
                            .unwrap();
                    }
                }
                if mode != "truncated" {
                    let _ = server.read(&mut request).await;
                }
            });
            let result = tokio::time::timeout(Duration::from_secs(2), async {
                let mut reader = connect_query(&runtime).await?;
                write_query(&mut reader, &runtime, Request::Ping, 1024).await?;
                let bytes = super::super::transport::framing::line(&mut reader, 1024).await?;
                decode_query_response(&bytes, &runtime)
            })
            .await
            .unwrap();
            match mode {
                "ok" => assert!(result.is_ok()),
                "oversize" => assert!(matches!(
                    result.unwrap_err().downcast_ref::<FrameError>(),
                    Some(FrameError::Oversize)
                )),
                "truncated" => assert!(matches!(
                    result.unwrap_err().downcast_ref::<FrameError>(),
                    Some(FrameError::Incomplete)
                )),
                "version" | "runtime" => assert!(matches!(
                    result.unwrap_err().downcast_ref::<FrameError>(),
                    Some(FrameError::Protocol)
                )),
                _ => assert!(result.is_err()),
            }
            tokio::time::timeout(Duration::from_secs(2), serving)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
