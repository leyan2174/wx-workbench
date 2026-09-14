//! 文本模式转义终端控制字符；JSONL 交由 serde 转义，不改写消息正文。
use super::Event;
use anyhow::Result;
use serde_json::Value;
use std::io::Write;

pub fn render(
    event: &Event,
    json: bool,
    max_content_chars: usize,
    writer: &mut impl Write,
) -> Result<()> {
    if json {
        serde_json::to_writer(&mut *writer, event)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        return Ok(());
    }
    let data = &event.data;
    let prefix = format!("[{} #{}]", event.observed_at, event.cycle);
    match event.kind {
        "started" => {
            writeln!(
                writer,
                "{prefix} {} 已开始；账号 {}；仅连接现有后台",
                if data["mode"] == "latency" {
                    "延迟观测"
                } else {
                    "消息监控"
                },
                event.runtime_id
            )?;
            writeln!(
                writer,
                "{}",
                terminal_text(&serde_json::to_string(data)?, 8192)
            )?;
        }
        "baseline" => writeln!(
            writer,
            "{prefix} 基线已建立，跟踪 {} 个会话；未回显历史",
            data["tracked_sessions"]
        )?,
        "messages" | "cursor_held" | "heartbeat" => {
            writeln!(
                writer,
                "{prefix} 返回 {} 条，limit={}，状态={}，游标{}；未证明完整追平",
                data["count"],
                data["limit"],
                field(data, "freshness_status", 128),
                if data["cursor_committed"].as_bool() == Some(true) {
                    "已接受"
                } else {
                    "保留"
                }
            )?;
            if data["possible_truncation"].as_bool() == Some(true) {
                writeln!(writer, "  可能达到截断边界；游标未推进，下一轮可能重复。可调整 max-limit（最高 10000）或显式使用 History 分页。")?;
            }
            if data["source_incomplete"].as_bool() == Some(true) {
                writeln!(
                    writer,
                    "  来源元数据缺失或提示不完整；保留游标，不将空结果视为完成。"
                )?;
            }
            if let Some(messages) = data["messages"].as_array() {
                for message in messages {
                    writeln!(
                        writer,
                        "  [{}] [{} / {}] {} [{}]: {}",
                        field(message, "time", 128),
                        field(message, "chat", 256),
                        field(message, "username", 512),
                        field(message, "sender", 256),
                        field(message, "type", 128),
                        field(message, "content", max_content_chars)
                    )?;
                }
            }
            if let Some(meta) = data.get("meta") {
                writeln!(
                    writer,
                    "  meta: {}",
                    terminal_text(&serde_json::to_string(meta)?, 8192)
                )?;
            }
        }
        "limit_increased" => writeln!(
            writer,
            "{prefix} 下一轮 limit={}；游标保持不变",
            data["limit"]
        )?,
        "cycle_error" | "latency_error" | "metadata_error" => {
            writeln!(
                writer,
                "{prefix} 本轮未完成，原状态保留：{}",
                terminal_text(&serde_json::to_string(data)?, 8192)
            )?;
        }
        "latency_interrupted" => {
            writeln!(
                writer,
                "{prefix} 当前探测被取消或观测窗口已结束；已完成阶段保留，原观测未推进：{}",
                terminal_text(&serde_json::to_string(data)?, 8192)
            )?;
        }
        "file_change" => {
            writeln!(
                writer,
                "{prefix} 本机文件变化：DB={} WAL={}；距上次观测 {} ms（不是网络延迟）",
                data["database_changed"],
                data["wal_changed"],
                ms(&data["observed_poll_gap_ms"])
            )?;
        }
        "latency_sample" => {
            writeln!(
                writer,
                "{prefix} IPC 采样总耗时 {} ms，文件元数据读取 {} ms",
                ms(&data["probe_total_ms"]),
                ms(&data["metadata_stat_ms"])
            )?;
            if let Some(phases) = data["phases"].as_object() {
                for name in ["ping", "sessions", "history"] {
                    let Some(phase) = phases.get(name) else {
                        continue;
                    };
                    let timing = phase.get("timing").unwrap_or(phase);
                    writeln!(writer, "  {name}: serialize={} connect={} write={} wait/read={} parse={} total={} ms",
                        ms(&timing["serialize_ms"]), ms(&timing["connect_ms"]), ms(&timing["write_ms"]), ms(&timing["wait_read_ms"]), ms(&timing["parse_ms"]), ms(&timing["total_ms"]))?;
                    if let Some(count) = phase.get("returned_count") {
                        writeln!(
                            writer,
                            "    返回 {count}，limit={}，可能截断={}，freshness={}",
                            phase["limit"],
                            phase["possible_truncation"],
                            field(&phase["meta"], "status", 128)
                        )?;
                        writeln!(
                            writer,
                            "    meta: {}",
                            terminal_text(&serde_json::to_string(&phase["meta"])?, 8192)
                        )?;
                    }
                }
            }
            if let Some(gap) = data["session_history_timestamp_gap_seconds"].as_i64() {
                writeln!(
                    writer,
                    "  session/history 时间戳差：{gap} 秒，仅用于新鲜度比较。"
                )?;
            }
            if !data["file_observation_to_probe_completion_ms"].is_null() {
                writeln!(
                    writer,
                    "  文件首次变化观测到本次探测完成：{} ms；未建立消息因果关系。",
                    ms(&data["file_observation_to_probe_completion_ms"])
                )?;
            }
            writeln!(
                writer,
                "  来源不完整={}；网络延迟、后台解密/纯查询耗时：当前协议不可观测。",
                data["source_incomplete"]
            )?;
        }
        "stopped" if data.get("requests").and_then(Value::as_object).is_some() => {
            writeln!(writer, "{prefix} 延迟观测已停止：{}；完成探测 {} 次，中断 {} 次，请求错误 {} 次，文件元数据错误 {} 次",
                field(data, "stop_reason", 128), data["successful_samples"], data["interrupted_samples"], data["query_errors"], data["metadata_errors"])?;
            writeln!(
                writer,
                "  来源不完整样本={}，可能截断样本={}，元数据不可用样本={}，待确认文件变化={}",
                data["source_incomplete_samples"],
                data["possible_truncation_samples"],
                data["metadata_unavailable_samples"],
                data["pending_file_change"]
            )?;
            for name in ["ping", "sessions", "history"] {
                let Some(request) = data["requests"].get(name) else {
                    continue;
                };
                writeln!(
                    writer,
                    "  {name}: 成功={} 失败={} 中断={}",
                    request["successful_requests"],
                    request["failed_requests"],
                    request["interrupted_requests"]
                )?;
                if request["successful_requests"].as_u64().unwrap_or(0) > 0 {
                    for phase_name in [
                        "serialize",
                        "connect",
                        "write",
                        "wait_read",
                        "parse",
                        "total",
                    ] {
                        let phase = &request["phases"][phase_name];
                        writeln!(
                            writer,
                            "    {phase_name}: 全程成功请求 min={} mean={} max={} ms",
                            ms(&phase["min_ms"]),
                            ms(&phase["mean_ms"]),
                            ms(&phase["max_ms"])
                        )?;
                    }
                    let recent = &request["recent_total"];
                    writeln!(
                        writer,
                        "    总耗时最近 {} 个成功请求（最多 {}）p50={} p95={} ms；nearest-rank",
                        recent["observations"],
                        recent["capacity"],
                        ms(&recent["p50_ms"]),
                        ms(&recent["p95_ms"])
                    )?;
                }
                if request["failed_requests"].as_u64().unwrap_or(0) > 0 {
                    writeln!(
                        writer,
                        "    失败分类：{}",
                        terminal_text(&serde_json::to_string(&request["failure_codes"])?, 2048)
                    )?;
                }
            }
            writeln!(writer, "  统计包含冷热缓存；成功探测不等于来源完整。网络延迟和后台内部解密/查询耗时未测量。")?;
        }
        "stopped" => writeln!(
            writer,
            "{prefix} 已停止：{}",
            terminal_text(&serde_json::to_string(data)?, 8192)
        )?,
        _ => writeln!(
            writer,
            "{prefix} {}: {}",
            event.kind,
            terminal_text(&serde_json::to_string(data)?, 8192)
        )?,
    }
    writer.flush()?;
    Ok(())
}

fn field(value: &Value, name: &str, maximum: usize) -> String {
    terminal_text(
        value.get(name).and_then(Value::as_str).unwrap_or(""),
        maximum,
    )
}
fn ms(value: &Value) -> String {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .map(|n| format!("{n:.3}"))
        .unwrap_or_else(|| "不可观测".into())
}

fn terminal_text(value: &str, maximum: usize) -> String {
    let mut output = String::new();
    let mut characters = value.chars();
    for ch in characters.by_ref().take(maximum) {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control()
                || matches!(ch, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                output.push_str(&format!("\\u{{{:x}}}", ch as u32))
            }
            ch => output.push(ch),
        }
    }
    if characters.next().is_some() {
        output.push_str(" [文本显示已截断；JSONL 可保留完整字段]");
    }
    output
}
