use crate::cli::transport;
use crate::cli::DaemonCommands;
use crate::ipc::Request;
use crate::runtime::RuntimeContext;
use anyhow::Result;

pub fn cmd_daemon(cmd: DaemonCommands) -> Result<()> {
    match cmd {
        DaemonCommands::Status => cmd_status(),
        DaemonCommands::Stop => cmd_stop(),
        DaemonCommands::Reload => cmd_reload(),
        DaemonCommands::Logs { follow, lines } => cmd_logs(follow, lines),
    }
}

fn cmd_reload() -> Result<()> {
    let response = transport::send(Request::ReloadConfig)?;
    let count = response
        .data
        .get("contacts")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!("已重新加载联系人缓存（{} 个）", count);
    Ok(())
}

fn cmd_status() -> Result<()> {
    if transport::is_alive() {
        let pid_path = RuntimeContext::load()?.pid_path();
        let pid = std::fs::read_to_string(&pid_path)
            .map(|s| {
                serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .and_then(|v| v.get("pid").and_then(|p| p.as_u64()))
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| s.trim().to_string())
            })
            .unwrap_or_else(|_| "?".into());
        println!("wx-daemon 运行中 (PID {})", pid);
    } else {
        println!("wx-daemon 未运行");
    }
    Ok(())
}

fn cmd_stop() -> Result<()> {
    // 不以 Ping 失败跳过停止：身份记录仍可安全识别未响应的后台。
    transport::stop_daemon()?;
    println!("当前账号 wx-daemon 已停止或未运行");
    Ok(())
}

fn cmd_logs(follow: bool, lines: usize) -> Result<()> {
    let log_path = RuntimeContext::load()?.log_path();
    if !log_path.exists() {
        println!("暂无日志");
        return Ok(());
    }

    if follow {
        #[cfg(windows)]
        {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(&log_path)?;
            let len = file.seek(SeekFrom::End(0))?;
            let start = len.saturating_sub((lines as u64) * 200);
            file.seek(SeekFrom::Start(start))?;
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            let all_lines: Vec<&str> = content.lines().collect();
            let show = &all_lines[all_lines.len().saturating_sub(lines)..];
            for line in show {
                println!("{}", line);
            }
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let mut buf = String::new();
                file.read_to_string(&mut buf)?;
                if !buf.is_empty() {
                    print!("{}", buf);
                }
            }
        }
    } else {
        let content = std::fs::read_to_string(&log_path)?;
        let all_lines: Vec<&str> = content.lines().collect();
        let show = &all_lines[all_lines.len().saturating_sub(lines)..];
        for line in show {
            println!("{}", line);
        }
    }

    Ok(())
}
