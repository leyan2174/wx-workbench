//! 详细消息解码的输出边界：一般错误 1、定位歧义 2。

use super::transport;
use crate::ipc::Request;
use anyhow::{Context, Result};
use std::io::Write;

pub fn cmd_decode(request: Request, json: bool) -> Result<()> {
    let response = transport::send(request)?;
    let code = response
        .data
        .get("exit_code")
        .and_then(|value| value.as_i64())
        .context("后台未返回消息解码状态")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    } else {
        println!(
            "{}",
            response
                .data
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
        );
    }
    if code != 0 {
        std::io::stdout().flush()?;
        std::process::exit(if code == 2 { 2 } else { 1 });
    }
    Ok(())
}
