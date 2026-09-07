//! delta 命令编排；账号查询经现有 IPC，文件发布只处理原始增量模型。
use crate::toolkit::chat_delta::{ContactMetadata, DeltaChat, DeltaRunWriter, DeltaWindow};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// 输出根目录；默认须全新，--append-run 可复用已有普通目录
    pub output: PathBuf,
    /// 精确 username，逗号分隔；默认 WECHAT_EXPORT_USERS 或全部会话
    #[arg(long)]
    pub users: Option<String>,
    /// 含端点的起始时间：本地日期、日期时间或 Unix 秒
    #[arg(long, allow_hyphen_values = true)]
    pub start: String,
    /// 含端点的结束时间；仅日期表示当天零点
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<String>,
    /// 批次目录名；默认本机时间及纳秒，不接受路径或设备名
    #[arg(long)]
    pub run_id: Option<String>,
    /// 在已有输出根目录中创建全新批次，不覆盖任何已有 run
    #[arg(long)]
    pub append_run: bool,
}

pub fn cmd(args: Args) -> Result<()> {
    let start = super::export_chats::parse_timestamp(&args.start)?;
    let end = args
        .end
        .as_deref()
        .map(super::export_chats::parse_timestamp)
        .transpose()?;
    ensure!(
        end.is_none_or(|end| start <= end),
        "起始时间不能晚于结束时间"
    );
    let now = chrono::Local::now();
    let window = DeltaWindow {
        start: Some(start),
        end,
        run_id: args
            .run_id
            .unwrap_or_else(|| now.format("%Y%m%d_%H%M%S_%9f").to_string()),
        utc_offset_seconds: now.offset().local_minus_utc(),
        generated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    // 时间、批次名及原始路径的校验必须先于账号读取和任何写入。
    window.validate()?;
    ensure!(
        !args
            .output
            .components()
            .any(|c| matches!(c, Component::ParentDir)),
        "输出路径不能包含 .."
    );
    let output = std::path::absolute(&args.output)?;
    let runtime = crate::runtime::RuntimeContext::load()?;
    super::export_chat::validate_output_for(&runtime, &output)?;
    let filter = args
        .users
        .unwrap_or_else(|| std::env::var("WECHAT_EXPORT_USERS").unwrap_or_default());
    let users: Vec<String> = if filter.trim().is_empty() {
        let response = super::transport::send_for(&runtime, crate::ipc::Request::ExportChatList)?;
        let targets: Vec<crate::message::export::Target> =
            serde_json::from_value(response.data["chats"].clone())?;
        targets.into_iter().map(|target| target.username).collect()
    } else {
        filter
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let dispatch = |query: DeltaQuery| {
        Ok(super::transport::send_for(
            &runtime,
            crate::ipc::Request::ExportDelta {
                username: query.username,
                start: query.start,
                end: query.end,
            },
        )?
        .data)
    };
    let report = if args.append_run {
        export_delta_with_mode(&output, &users, window, true, dispatch)?
    } else {
        cmd_export_delta(&output, &users, window, dispatch)?
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(
        report["success"] == true,
        "部分增量会话导出失败，详见 manifest"
    );
    Ok(())
}

/// dispatcher 可直接映射为 Request::ExportDelta { username, start, end }。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaQuery {
    pub username: String,
    pub start: i64,
    pub end: Option<i64>,
}

/// 请求响应必须是完整 DeltaChat JSON；summary/history 响应会明确失败。
/// 返回 success=false 仍有已落盘的失败 manifest；调用者据此设置非零退出码。
pub fn cmd_export_delta<F>(
    output_root: &Path,
    usernames: &[String],
    window: DeltaWindow,
    dispatch: F,
) -> Result<Value>
where
    F: FnMut(DeltaQuery) -> Result<Value>,
{
    export_delta_with_mode(output_root, usernames, window, false, dispatch)
}

pub(super) fn export_delta_with_mode<F>(
    output_root: &Path,
    usernames: &[String],
    window: DeltaWindow,
    append_run: bool,
    mut dispatch: F,
) -> Result<Value>
where
    F: FnMut(DeltaQuery) -> Result<Value>,
{
    window.validate()?;
    ensure!(
        !usernames.is_empty(),
        "delta export requires explicit usernames"
    );
    ensure!(usernames.iter().all(|s| !s.is_empty()), "username 不能为空");
    let start = window.start.context("delta export requires start_ts")?;
    let end = window.end;
    let mut writer = if append_run {
        DeltaRunWriter::create_run_in_existing_root(output_root, window)?
    } else {
        DeltaRunWriter::create(output_root, window)?
    };
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for username in usernames {
        if !seen.insert(username) {
            continue;
        }
        let response = (|| -> Result<(DeltaChat, Option<Value>)> {
            let value = dispatch(DeltaQuery {
                username: username.clone(),
                start,
                end,
            })?;
            let warnings = value.get("metadata_warnings").cloned();
            let chat: DeltaChat = serde_json::from_value(value)
                .context("delta IPC response must contain raw DeltaChat fields")?;
            ensure!(chat.username == *username, "delta IPC username mismatch");
            Ok((chat, warnings))
        })();
        let (chat, warnings) = response.unwrap_or_else(|error| {
            (
                DeltaChat {
                    username: username.clone(),
                    display_name: username.clone(),
                    is_group: username.ends_with("@chatroom"),
                    contact: ContactMetadata::default(),
                    messages: Vec::new(),
                    source_error: Some(format!("delta query error: {error:#}")),
                },
                None,
            )
        });
        let mut result = writer.write_chat(&chat);
        if let Some(warnings) = warnings {
            result["metadata_warnings"] = warnings;
        }
        results.push(result);
    }
    let manifest = writer.finish()?;
    Ok(json!({
        "success": results.iter().all(|result| result["success"] == true),
        "manifest_path": manifest,
        "chats_checked": results.len(),
        "messages_exported": results.iter().filter_map(|result| result["message_count"].as_u64()).sum::<u64>(),
        "results": results,
    }))
}

#[cfg(test)]
#[path = "../../tests/fixtures/delta-query/cli_tests.rs"]
mod tests;
