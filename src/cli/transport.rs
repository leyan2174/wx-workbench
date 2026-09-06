use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config;
use crate::ipc::{Request, Response};

const STARTUP_TIMEOUT_SECS: u64 = 15;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PidFile {
    pid: u32,
    #[serde(default)]
    exe: Option<PathBuf>,
}

/// 检查 daemon 是否存活
pub fn is_alive() -> bool {
    ping_windows().unwrap_or(false)
}

/// 确保 daemon 运行，必要时自动启动
pub fn ensure_daemon() -> Result<()> {
    if is_alive() {
        return Ok(());
    }
    eprintln!("启动 wx-daemon...");
    start_daemon()?;
    Ok(())
}

/// 停止 daemon（如果正在运行）
pub fn stop_daemon() -> Result<()> {
    let pid_path = config::pid_path();
    let pid_file = read_pid_file(&pid_path)?;
    let daemon_alive = is_alive();

    match pid_file {
        Some(pid_file) => {
            let belongs = pid_belongs_to_daemon(&pid_file)?;
            if daemon_alive && !belongs {
                bail!(
                    "daemon 正在运行，但 {} 指向的 PID {} 无法确认属于当前 wx-daemon",
                    pid_path.display(),
                    pid_file.pid
                );
            }
            if belongs {
                terminate_pid(pid_file.pid)?;
            }
        }
        None if daemon_alive => {
            bail!(
                "daemon 正在运行，但 {} 缺失或损坏，无法安全停止",
                pid_path.display()
            );
        }
        None => {}
    }

    cleanup_ipc_files();
    Ok(())
}

/// Check the runtime directory before starting the daemon.
fn preflight_cli_dir_writable() -> Result<()> {
    let cli_dir = config::cli_dir();
    std::fs::create_dir_all(&cli_dir)
        .with_context(|| format!("创建 {} 失败", cli_dir.display()))?;

    let probe = cli_dir.join(".daemon_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let dir = cli_dir.display();
            bail!("无法写入 {dir}: {e}");
        }
        Err(e) => bail!("无法写入 {}: {}", cli_dir.display(), e),
    }
}

/// 启动 daemon 进程（自身二进制，设置 WX_DAEMON_MODE=1）
fn start_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;
    let child_pid: u32;

    // 预检：当前用户是否能写 ~/.wx-cli/。如果不能，给出可操作的错误信息，
    // 而不是 spawn 一个注定失败的 daemon 然后超时 15s。
    preflight_cli_dir_writable()?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let log_path = config::log_path();
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (stdout_stdio, stderr_stdio) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|f| f.try_clone().map(|g| (f, g)))
            .map(|(f, g)| (std::process::Stdio::from(f), std::process::Stdio::from(g)))
            .unwrap_or_else(|_| (std::process::Stdio::null(), std::process::Stdio::null()));
        let child = std::process::Command::new(&exe)
            .env("WX_DAEMON_MODE", "1")
            .stdin(std::process::Stdio::null())
            .stdout(stdout_stdio)
            .stderr(stderr_stdio)
            .creation_flags(0x00000008) // DETACHED_PROCESS
            .spawn()
            .context("无法启动 daemon 进程")?;
        child_pid = child.id();
    }

    // 等待 daemon 就绪（最多 STARTUP_TIMEOUT_SECS 秒）
    let deadline = std::time::Instant::now() + Duration::from_secs(STARTUP_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if is_alive() {
            write_pid_file(child_pid, &exe)?;
            return Ok(());
        }
    }

    bail!(
        "wx-daemon 启动超时（>{}s）\n请查看日志: {}",
        STARTUP_TIMEOUT_SECS,
        config::log_path().display()
    )
}

fn write_pid_file(pid: u32, exe: &Path) -> Result<()> {
    if let Some(parent) = config::pid_path().parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 {} 失败", parent.display()))?;
    }
    let pid_file = PidFile {
        pid,
        exe: Some(exe.to_path_buf()),
    };
    let content = serde_json::to_string(&pid_file)?;
    std::fs::write(config::pid_path(), content)
        .with_context(|| format!("写入 {} 失败", config::pid_path().display()))?;
    Ok(())
}

fn read_pid_file(path: &Path) -> Result<Option<PidFile>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("读取 {} 失败", path.display())),
    };
    if let Ok(pid_file) = serde_json::from_str::<PidFile>(&content) {
        return Ok(Some(pid_file));
    }
    if let Ok(pid) = content.trim().parse::<u32>() {
        return Ok(Some(PidFile {
            pid,
            exe: std::env::current_exe().ok(),
        }));
    }
    bail!("{} 不是合法的 PID 文件", path.display())
}

fn cleanup_ipc_files() {
    let _ = std::fs::remove_file(config::pid_path());
}

#[cfg(windows)]
fn ping_windows() -> Result<bool> {
    use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

    let name = "wx-cli-daemon".to_ns_name::<GenericNamespaced>()?;
    let stream = Stream::connect(name)?;
    let mut reader = BufReader::new(stream);

    let req = serde_json::to_string(&Request::Ping)? + "\n";
    reader.get_mut().write_all(req.as_bytes())?;

    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: Response = serde_json::from_str(&line)?;
    Ok(resp.ok && resp.data.get("pong").and_then(|p| p.as_bool()) == Some(true))
}

fn pid_belongs_to_daemon(pid_file: &PidFile) -> Result<bool> {
    let expected_exe = pid_file
        .exe
        .clone()
        .or_else(|| std::env::current_exe().ok());
    windows_pid_matches_daemon(pid_file.pid, expected_exe.as_deref())
}

#[cfg(windows)]
fn windows_pid_matches_daemon(pid: u32, expected_exe: Option<&Path>) -> Result<bool> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let Some(expected_exe) = expected_exe else {
        return Ok(false);
    };
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        Err(_) => return Ok(false),
    };

    let mut buf = vec![0u16; 260];
    let mut len = buf.len() as u32;
    let actual = unsafe {
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result
    };
    if actual.is_err() {
        return Ok(false);
    }

    let actual_path = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
    Ok(normalize_exe_path(&actual_path) == normalize_exe_path(expected_exe))
}

#[cfg(windows)]
fn normalize_exe_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn terminate_pid(pid: u32) -> Result<()> {
    terminate_pid_windows(pid)
}

#[cfg(windows)]
fn terminate_pid_windows(pid: u32) -> Result<()> {
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .with_context(|| format!("执行 taskkill /PID {} 失败", pid))?;
    if !status.success() {
        bail!("停止 PID {} 失败: taskkill exit {:?}", pid, status.code());
    }
    Ok(())
}

/// 向 daemon 发送请求并返回响应
pub fn send(req: Request) -> Result<Response> {
    ensure_daemon()?;

    send_windows(req)
}

#[cfg(windows)]
fn send_windows(req: Request) -> Result<Response> {
    use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

    let name = "wx-cli-daemon"
        .to_ns_name::<GenericNamespaced>()
        .context("构造 pipe name 失败")?;
    let stream = Stream::connect(name).context("连接 daemon named pipe 失败")?;

    // interprocess::Stream 同时实现 Read + Write，但需要拆分读写端
    let mut reader = BufReader::new(stream);

    let req_str = serde_json::to_string(&req)? + "\n";
    reader.get_mut().write_all(req_str.as_bytes())?;

    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: Response = serde_json::from_str(&line).context("解析 daemon 响应失败")?;

    if !resp.ok {
        bail!("{}", resp.error.as_deref().unwrap_or("未知错误"));
    }

    Ok(resp)
}
