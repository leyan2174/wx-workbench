use anyhow::{Context, Result};
use std::path::PathBuf;

use super::toolkit;
use super::transport;

pub fn cmd_sns_album(
    user: String,
    output_root: String,
    limit: usize,
    since: Option<String>,
    until: Option<String>,
    no_remote: bool,
) -> Result<()> {
    transport::ensure_daemon()?;
    let wx_exe = std::env::current_exe().context("无法定位当前 wx 可执行文件")?;
    let output_root = absolute_path(PathBuf::from(output_root))?;
    let mut args = vec![
        "--wx-exe".to_string(),
        wx_exe.to_string_lossy().into_owned(),
        "--user".to_string(),
        user,
        "--output-root".to_string(),
        output_root.to_string_lossy().into_owned(),
        "--limit".to_string(),
        limit.to_string(),
    ];
    push_option(&mut args, "--since", since);
    push_option(&mut args, "--until", until);
    if no_remote {
        args.push("--no-remote".to_string());
    }
    toolkit::run_script("export_sns_album.py", args, None)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()
        .context("无法读取当前目录")?
        .join(path))
}

fn push_option(args: &mut Vec<String>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(name.to_string());
        args.push(value);
    }
}
