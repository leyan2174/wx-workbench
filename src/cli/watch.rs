use anyhow::Result;
use std::io::BufRead;

use crate::ipc::Request;
use super::super::cli::transport;

pub fn cmd_watch(chat: Option<String>, json: bool) -> Result<()> {
    transport::ensure_daemon()?;

    let sock_path = crate::config::sock_path();

    // 连接 socket
    #[cfg(unix)]
    let mut stream = {
        use std::os::unix::net::UnixStream;
        UnixStream::connect(&sock_path)?
    };

    // 发送 watch 请求
    let req_line = serde_json::to_string(&Request::Watch)? + "\n";
    #[cfg(unix)]
    {
        use std::io::Write;
        stream.write_all(req_line.as_bytes())?;
    }

    if !json {
        eprintln!("监听中（Ctrl+C 退出）...\n");
    }

    #[cfg(windows)]
    {
        anyhow::bail!("watch 命令在 Windows 上暂不支持，请使用 Unix 系统");
    }

    #[cfg(unix)]
    {
        let reader = std::io::BufReader::new(stream.try_clone()?);
        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(_) => break,
            };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let event: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let evt = event["event"].as_str().unwrap_or("");
            if evt == "connected" || evt == "heartbeat" {
                continue;
            }

            // 过滤指定聊天
            if let Some(ref filter_chat) = chat {
                let event_chat = event["chat"].as_str().unwrap_or("");
                let event_user = event["username"].as_str().unwrap_or("");
                if event_chat != filter_chat && event_user != filter_chat {
                    continue;
                }
            }

            if json {
                println!("{}", line);
                continue;
            }

            let time_s = event["time"].as_str().unwrap_or("");
            let chat_s = event["chat"].as_str().unwrap_or("");
            let is_group = event["is_group"].as_bool().unwrap_or(false);
            let sender = event["sender"].as_str().unwrap_or("");
            let content = event["content"].as_str().unwrap_or("");

            let chat_part = if is_group {
                format!("\x1b[36m[{}]\x1b[0m ", chat_s)
            } else {
                format!("\x1b[1m{}\x1b[0m ", chat_s)
            };
            let sender_part = if !sender.is_empty() {
                format!("\x1b[33m{}\x1b[0m: ", sender)
            } else {
                String::new()
            };

            println!("\x1b[90m[{}]\x1b[0m {}{}{}", time_s, chat_part, sender_part, content);
        }
    }

    Ok(())
}
