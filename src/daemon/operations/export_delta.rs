//! delta 命令编排；账号查询经现有 IPC，文件发布只处理原始增量模型。
use crate::business::archive::{self, DeltaPublisher, DeltaSource, Failure, Publication, Stage};
use crate::toolkit::chat_delta::{ContactMetadata, DeltaChat, DeltaRunWriter, DeltaWindow};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub use crate::service::operation_requests::export_delta::Args;

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
        let response =
            crate::service::query_client::send_for(&runtime, crate::ipc::Request::ExportChatList)?;
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
        Ok(crate::service::query_client::send_for(
            &runtime,
            crate::ipc::Request::ExportDelta {
                username: query.username,
                start: query.start,
                end: query.end,
            },
        )?
        .data)
    };
    let report = export_delta_with_mode(
        &output,
        &users,
        window,
        args.append_run,
        &crate::toolkit::export_protected(&runtime),
        dispatch,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    let results = report["results"]
        .as_array()
        .context("Invalid delta export report")?;
    let succeeded = results
        .iter()
        .filter(|item| item["success"] == true)
        .count() as u64;
    crate::ipc::outcome::BusinessOutcome::from_counts(succeeded, results.len() as u64 - succeeded)
        .require_success()?;
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
#[cfg(test)]
pub fn cmd_export_delta<F>(
    output_root: &Path,
    usernames: &[String],
    window: DeltaWindow,
    dispatch: F,
) -> Result<Value>
where
    F: FnMut(DeltaQuery) -> Result<Value>,
{
    export_delta_with_mode(output_root, usernames, window, false, &[], dispatch)
}

pub(super) fn export_delta_with_mode<F>(
    output_root: &Path,
    usernames: &[String],
    window: DeltaWindow,
    append_run: bool,
    protected: &[PathBuf],
    dispatch: F,
) -> Result<Value>
where
    F: FnMut(DeltaQuery) -> Result<Value>,
{
    window.validate()?;
    let plan = archive::DeltaPlan::new(
        usernames,
        archive::Range {
            start: window.start.context("delta export requires start_ts")?,
            end: window.end,
        },
    )?;
    let writer = if append_run {
        DeltaRunWriter::append_protected(output_root, window, protected)?
    } else {
        DeltaRunWriter::create_protected(output_root, window, protected)?
    };
    let completed = archive::export_delta(
        plan,
        QuerySource(dispatch),
        Output {
            writer,
            results: Vec::new(),
        },
    )?;
    Ok(json!({
        "success": completed.success(),
        "manifest_path": completed.manifest.0,
        "chats_checked": completed.targets.len(),
        "messages_exported": completed.messages(),
        "results": completed.manifest.1,
    }))
}

// The legacy raw-export document is intentionally confined to the IPC/format boundary.
struct RawDelta {
    chat: DeltaChat,
    warnings: Option<Value>,
}

struct QuerySource<F>(F);
impl<F: FnMut(DeltaQuery) -> Result<Value>> DeltaSource for QuerySource<F> {
    type Document = RawDelta;

    fn read(
        &mut self,
        username: &str,
        range: archive::Range,
    ) -> std::result::Result<archive::RawArchive<RawDelta>, Failure> {
        let read = (|| -> Result<_> {
            let value = (self.0)(DeltaQuery {
                username: username.into(),
                start: range.start,
                end: range.end,
            })?;
            let warnings = value.get("metadata_warnings").cloned();
            let chat: DeltaChat = serde_json::from_value(value)
                .context("delta IPC response must contain raw DeltaChat fields")?;
            Ok(archive::RawArchive {
                username: chat.username.clone(),
                document: RawDelta { chat, warnings },
            })
        })();
        read.map_err(|error| Failure {
            stage: Stage::Read,
            detail: format!("delta query error: {error:#}"),
        })
    }
}

struct Output {
    writer: DeltaRunWriter,
    results: Vec<Value>,
}

impl DeltaPublisher<RawDelta> for Output {
    type Manifest = (PathBuf, Vec<Value>);

    fn record(
        &mut self,
        username: &str,
        document: std::result::Result<RawDelta, Failure>,
    ) -> Publication {
        let raw = document.unwrap_or_else(|failure| RawDelta {
            chat: DeltaChat {
                username: username.into(),
                display_name: username.into(),
                is_group: username.ends_with("@chatroom"),
                contact: ContactMetadata::default(),
                messages: Vec::new(),
                source_error: Some(failure.detail),
            },
            warnings: None,
        });
        let source_error = raw.chat.source_error.is_some();
        let mut result = self.writer.write_chat(&raw.chat);
        if let Some(warnings) = raw.warnings {
            result["metadata_warnings"] = warnings;
        }
        let publication = if result["success"] != true {
            Publication::Failed(Failure {
                stage: if source_error {
                    Stage::Read
                } else {
                    Stage::Publish
                },
                detail: result["reason"]
                    .as_str()
                    .unwrap_or("invalid delta publication result")
                    .into(),
            })
        } else if result["skipped"] == true {
            Publication::Skipped
        } else if let Some(messages) = result["message_count"].as_u64() {
            Publication::Written { messages }
        } else {
            Publication::Failed(Failure {
                stage: Stage::Publish,
                detail: "missing delta message count".into(),
            })
        };
        self.results.push(result);
        publication
    }

    fn finish(self) -> std::result::Result<Self::Manifest, Failure> {
        self.writer
            .finish()
            .map(|path| (path, self.results))
            .map_err(|error| Failure {
                stage: Stage::Manifest,
                detail: format!("{error:#}"),
            })
    }
}

#[cfg(test)]
#[path = "../../../tests/fixtures/delta-query/cli_tests.rs"]
mod tests;
