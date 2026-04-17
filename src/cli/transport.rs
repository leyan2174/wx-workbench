use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::config;
use crate::ipc::{Request, Response};

const STARTUP_TIMEOUT_SECS: u64 = 15;

/// 检查 daemon 是否存活
pub fn is_alive() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let sock_path = config::sock_path();
        if !sock_path.exists() {
            return false;
        }
        let mut stream = match UnixStream::connect(&sock_path) {
            Ok(s) => s,
            Err(_) => return false,
        };
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(2))).ok();

        let req = serde_json::json!({"cmd": "ping"});
        if write!(stream, "{}\n", req).is_err() {
            return false;
        }
        let mut line = String::new();
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut line).is_err() {
            return false;
        }
        serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("pong").and_then(|p| p.as_bool()))
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        // 通过 named pipe 检测
        let pipe_path = r"\\.\pipe\wx-cli-daemon";
        use std::fs::OpenOptions;
        OpenOptions::new().read(true).write(true).open(pipe_path).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
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

/// 启动 daemon 进程（自身二进制，设置 WX_DAEMON_MODE=1）
fn start_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 日志文件：~/.wx-cli/daemon.log
        let log_path = config::log_path();
        // 确保父目录存在
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (stdout_stdio, stderr_stdio) = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open(&log_path)
            .and_then(|f| f.try_clone().map(|g| (f, g)))
            .map(|(f, g)| (std::process::Stdio::from(f), std::process::Stdio::from(g)))
            .unwrap_or_else(|_| (std::process::Stdio::null(), std::process::Stdio::null()));
        let mut cmd = std::process::Command::new(&exe);
        cmd.env("WX_DAEMON_MODE", "1")
            .stdin(std::process::Stdio::null())
            .stdout(stdout_stdio)
            .stderr(stderr_stdio);
        // SAFETY: setsid() 在 fork 后的子进程中调用，使 daemon 脱离控制终端
        unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
        let _ = cmd.spawn().context("无法启动 daemon 进程")?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let log_path = config::log_path();
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (stdout_stdio, stderr_stdio) = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open(&log_path)
            .and_then(|f| f.try_clone().map(|g| (f, g)))
            .map(|(f, g)| (std::process::Stdio::from(f), std::process::Stdio::from(g)))
            .unwrap_or_else(|_| (std::process::Stdio::null(), std::process::Stdio::null()));
        let _ = std::process::Command::new(&exe)
            .env("WX_DAEMON_MODE", "1")
            .stdin(std::process::Stdio::null())
            .stdout(stdout_stdio)
            .stderr(stderr_stdio)
            .creation_flags(0x00000008) // DETACHED_PROCESS
            .spawn()
            .context("无法启动 daemon 进程")?;
    }

    // 等待 daemon 就绪（最多 STARTUP_TIMEOUT_SECS 秒）
    let deadline = std::time::Instant::now() + Duration::from_secs(STARTUP_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if is_alive() {
            return Ok(());
        }
    }

    bail!(
        "wx-daemon 启动超时（>{}s）\n请查看日志: {}",
        STARTUP_TIMEOUT_SECS,
        config::log_path().display()
    )
}

/// 向 daemon 发送请求并返回响应
pub fn send(req: Request) -> Result<Response> {
    ensure_daemon()?;

    #[cfg(unix)]
    {
        send_unix(req)
    }
    #[cfg(windows)]
    {
        send_windows(req)
    }
    #[cfg(not(any(unix, windows)))]
    {
        bail!("不支持当前平台")
    }
}

#[cfg(unix)]
fn send_unix(req: Request) -> Result<Response> {
    use std::os::unix::net::UnixStream;
    let sock_path = config::sock_path();
    let mut stream = UnixStream::connect(&sock_path)
        .context("连接 daemon socket 失败")?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(120))).ok();

    let req_str = serde_json::to_string(&req)? + "\n";
    stream.write_all(req_str.as_bytes())?;

    let mut line = String::new();
    let mut reader = BufReader::new(&stream);
    reader.read_line(&mut line)?;

    let resp: Response = serde_json::from_str(&line)
        .context("解析 daemon 响应失败")?;

    if !resp.ok {
        bail!("{}", resp.error.as_deref().unwrap_or("未知错误"));
    }

    Ok(resp)
}

#[cfg(windows)]
fn send_windows(req: Request) -> Result<Response> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let pipe_path = r"\\.\pipe\wx-cli-daemon";
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path)
        .context("连接 daemon named pipe 失败")?;

    let req_str = serde_json::to_string(&req)? + "\n";
    pipe.write_all(req_str.as_bytes())?;

    let mut line = String::new();
    let mut reader = BufReader::new(pipe);
    reader.read_line(&mut line)?;

    let resp: Response = serde_json::from_str(&line)
        .context("解析 daemon 响应失败")?;

    if !resp.ok {
        bail!("{}", resp.error.as_deref().unwrap_or("未知错误"));
    }

    Ok(resp)
}
