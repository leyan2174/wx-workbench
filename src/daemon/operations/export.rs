use super::history::{parse_time, parse_time_end};
use super::output::{emit_warnings, warning_block_markdown, warning_block_text, OutputOpts};
use crate::service::query_client as transport;
use crate::{
    ipc::Request,
    runtime::RuntimeContext,
    service::history_export::{Format, HistoryQuerySummary, MAX_OUTPUT_BYTES},
};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

pub fn cmd_export(
    chat: String,
    since: Option<String>,
    until: Option<String>,
    limit: usize,
    format: String,
    output: Option<String>,
    opts: OutputOpts,
) -> Result<()> {
    let format: Format = serde_json::from_value(serde_json::Value::String(format))?;
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;
    let runtime = RuntimeContext::load()?;
    let target = output
        .as_ref()
        .map(|path| {
            crate::infrastructure::publication::ExportTarget::capture(&runtime, Path::new(path))
        })
        .transpose()?;
    let resp = transport::send_for(
        &runtime,
        history_request(chat, since_ts, until_ts, limit, opts),
    )?;
    resp.require_success()?;
    let view = history_data(&resp.data, limit)?;
    emit_warnings(&resp.data);
    let text = render_history(&resp.data, &view, format)?;
    match output {
        Some(path) => {
            target
                .expect("file target captured before query")
                .write_bytes(text.as_bytes())?;
            println!("已导出 {} 条消息到 {}", view.messages.len(), path);
        }
        None => println!("{}", text),
    }
    Ok(())
}

fn history_request(
    chat: String,
    since: Option<i64>,
    until: Option<i64>,
    limit: usize,
    opts: OutputOpts,
) -> Request {
    let (with_meta, debug_source) = opts.request_flags();
    Request::History {
        chat,
        limit,
        offset: 0,
        since,
        until,
        msg_type: None,
        msg_types: None,
        oldest_first: false,
        with_meta,
        debug_source,
    }
}

struct HistoryData<'a> {
    username: &'a str,
    chat: &'a str,
    is_group: bool,
    messages: &'a [serde_json::Value],
}

fn history_data(data: &serde_json::Value, limit: usize) -> Result<HistoryData<'_>> {
    let username = data["username"]
        .as_str()
        .context("Missing history username")?;
    ensure!(
        !username.is_empty() && username.len() <= 256 && !username.chars().any(char::is_control),
        "Invalid history username"
    );
    let chat = data["chat"].as_str().context("Missing history chat")?;
    let is_group = data["is_group"]
        .as_bool()
        .context("Missing history group flag")?;
    let messages = data["messages"]
        .as_array()
        .context("Missing history messages")?;
    ensure!(
        data["count"].as_u64() == Some(messages.len() as u64)
            && messages.len() <= limit
            && data["meta"].is_object(),
        "Invalid history response"
    );
    for message in messages {
        ensure!(
            message.is_object()
                && message["time"].is_string()
                && message["sender"].is_string()
                && message["content"].is_string(),
            "Invalid history message"
        );
    }
    Ok(HistoryData {
        username,
        chat,
        is_group,
        messages,
    })
}

pub(super) fn export_task_for(
    runtime: &RuntimeContext,
    id: &str,
    request: crate::service::history_export::Request,
    window: (Option<i64>, Option<i64>),
    output: &Path,
) -> Result<()> {
    ensure!(
        request.resolved_window()? == window,
        "History time selection changed"
    );
    ensure!(
        output == crate::daemon::tasks::history_artifacts::output_path(runtime, id, &request)?,
        "History task output mismatch"
    );
    let pin = crate::service::config_pin::ConfigPin::new(runtime)?;
    let target = crate::infrastructure::publication::ExportTarget::new_file(
        output,
        &crate::infrastructure::publication::export_protected(runtime),
    )?;
    let response = transport::send_with_limits(
        runtime,
        history_request(
            request.chat.clone(),
            window.0,
            window.1,
            request.limit,
            OutputOpts {
                json: false,
                with_meta: false,
                debug_source: false,
            },
        ),
        Duration::from_secs(300),
        crate::ipc::QUERY_RESPONSE_LIMIT,
    )?;
    response.require_success()?;
    let view = history_data(&response.data, request.limit)?;
    let text = render_history(&response.data, &view, request.format)?;
    ensure!(
        text.len() <= MAX_OUTPUT_BYTES,
        "History export exceeds output limit"
    );
    let summary = HistoryQuerySummary {
        username: view.username.into(),
        since_ts: window.0,
        until_ts: window.1,
        limit: request.limit,
        messages: view.messages.len() as u64,
    };
    let warnings = crate::service::output::warning_lines(&response.data).len() as u64;
    target.write_bytes_checked(text.as_bytes(), || pin.verify(runtime))?;
    pin.verify(runtime)?;
    crate::daemon::tasks::history_artifacts::published(
        runtime,
        id,
        &request,
        summary,
        warnings,
        &format!("{:x}", Sha256::digest(text.as_bytes())),
    )
}

fn render_history(
    data: &serde_json::Value,
    view: &HistoryData<'_>,
    format: Format,
) -> Result<String> {
    let messages = view.messages;
    let chat_name = view.chat;
    let is_group = view.is_group;
    let count = messages.len();
    let text = match format {
        Format::Json => serde_json::to_string_pretty(data)?,
        Format::Yaml => serde_yaml::to_string(data)?,
        Format::Txt => {
            let group_str = if is_group { "[群]" } else { "" };
            let mut lines = vec![format!(
                "=== {}{} ({} 条) ===\n",
                chat_name, group_str, count
            )];
            if let Some(warn) = warning_block_text(data) {
                lines.push(warn);
                lines.push(String::new());
            }
            for m in messages {
                let time = m["time"].as_str().unwrap_or("");
                let sender = m["sender"].as_str().unwrap_or("");
                let content = m["content"].as_str().unwrap_or("");
                let sender_str = if !sender.is_empty() {
                    format!("{}: ", sender)
                } else {
                    String::new()
                };
                lines.push(format!("[{}] {}{}", time, sender_str, content));
            }
            lines.join("\n")
        }
        Format::Markdown => {
            // markdown (default)
            let group_str = if is_group { "（群聊）" } else { "" };
            let mut lines = vec![
                format!("# {}{}", chat_name, group_str),
                format!("\n> 导出 {} 条消息\n", count),
            ];
            if let Some(warn) = warning_block_markdown(data) {
                lines.push(warn);
            }
            for m in messages {
                let time = m["time"].as_str().unwrap_or("");
                let sender = m["sender"].as_str().unwrap_or("");
                let content = m["content"].as_str().unwrap_or("").replace('\n', "\n> ");
                let sender_md = if !sender.is_empty() {
                    format!("**{}**: ", sender)
                } else {
                    String::new()
                };
                lines.push(format!("### {}\n\n{}{}\n", time, sender_md, content));
            }
            lines.join("\n")
        }
    };

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data() -> serde_json::Value {
        json!({"username":"peer", "chat":"合成会话", "is_group":false, "count":1,
            "messages":[{"time":"2026-09-18 12:00:00","sender":"甲","content":"第一行\n第二行"}],
            "meta":{}, "extra_legacy_field":"preserved"})
    }

    #[test]
    fn four_formats_share_legacy_rendering_and_keep_complete_json() {
        let data = data();
        let view = history_data(&data, 500).unwrap();
        assert_eq!(
            render_history(&data, &view, Format::Json).unwrap(),
            serde_json::to_string_pretty(&data).unwrap()
        );
        assert_eq!(
            render_history(&data, &view, Format::Yaml).unwrap(),
            serde_yaml::to_string(&data).unwrap()
        );
        assert_eq!(
            render_history(&data, &view, Format::Txt).unwrap(),
            "=== 合成会话 (1 条) ===\n\n[2026-09-18 12:00:00] 甲: 第一行\n第二行"
        );
        assert_eq!(render_history(&data, &view, Format::Markdown).unwrap(), "# 合成会话\n\n> 导出 1 条消息\n\n### 2026-09-18 12:00:00\n\n**甲**: 第一行\n> 第二行\n");
    }

    #[test]
    fn malformed_success_is_not_a_valid_empty_history() {
        for field in ["username", "chat", "is_group", "count", "messages", "meta"] {
            let mut data = data();
            data.as_object_mut().unwrap().remove(field);
            assert!(history_data(&data, 500).is_err(), "{field}");
        }
        let mut data = data();
        data["count"] = json!(0);
        assert!(history_data(&data, 500).is_err());
        data["messages"] = json!([]);
        let empty = history_data(&data, 500).unwrap();
        assert!(empty.messages.is_empty());
        assert!(!render_history(&data, &empty, Format::Markdown)
            .unwrap()
            .is_empty());
        data["messages"] = json!([{"time": "now", "sender": "a", "content": null}]);
        data["count"] = json!(1);
        assert!(history_data(&data, 500).is_err());
    }

    #[test]
    fn business_failure_and_ambiguity_cannot_reach_rendering() {
        for response in [
            crate::ipc::Response {
                ok: false,
                error: Some("ambiguous_chat".into()),
                data: data(),
            },
            crate::ipc::Response {
                ok: false,
                error: None,
                data: data(),
            },
            crate::ipc::Response {
                ok: true,
                error: Some("query failed".into()),
                data: data(),
            },
        ] {
            assert!(response.require_success().is_err());
        }
        let request = history_request(
            "orphan-peer".into(),
            Some(1),
            Some(2),
            10001,
            OutputOpts {
                json: false,
                with_meta: false,
                debug_source: false,
            },
        );
        assert!(
            matches!(request, Request::History {chat, limit:10001, offset:0,
            since:Some(1), until:Some(2), oldest_first:false, with_meta:false, debug_source:false, ..} if chat == "orphan-peer")
        );
    }
}
