//! 原生终端监控和延迟诊断，只连接一次固定的账号上下文，不启动/停止后台。
//! 秒级 IPC 游标不是可靠消息队列；不能承诺同秒迟到消息或完整历史均已送达。

pub mod latency;
mod state;
mod statistics;
mod terminal;
mod transport;

pub use state::StateFile;
pub use terminal::render;
pub use transport::{FixedRuntimeContext, RequestTiming};

use crate::{infrastructure::cancellation::Cancellation, ipc::Request};
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

pub const CURSOR_WARNING: &str = "timestamp_seconds_strict_gt_cannot_prove_lossless_delivery";
pub const MAX_SESSIONS: usize = crate::service::protocol::monitor::MAX_SESSIONS;
pub const MAX_STATE_BYTES: u64 = crate::service::protocol::monitor::MAX_STATE_BYTES as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitialPolicy {
    Now,
    Recent,
}

pub struct MonitorOptions {
    pub interval: Duration,
    pub request_timeout: Duration,
    pub limit: usize,
    pub max_limit: usize,
    pub max_response_bytes: usize,
    pub initial: InitialPolicy,
    /// 只读恢复输入；不创建、不覆盖、不自动迁移旧版 last_check.json。
    pub state_file: Option<PathBuf>,
    pub with_meta: bool,
    pub debug_source: bool,
    pub emit_state: bool,
    pub max_cycles: Option<u64>,
    pub max_duration: Option<Duration>,
    pub max_consecutive_errors: Option<u64>,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3),
            request_timeout: Duration::from_secs(20),
            limit: 200,
            max_limit: 10_000,
            max_response_bytes: 8 * 1024 * 1024,
            initial: InitialPolicy::Now,
            state_file: None,
            with_meta: false,
            debug_source: false,
            emit_state: false,
            max_cycles: None,
            max_duration: None,
            max_consecutive_errors: None,
        }
    }
}

impl MonitorOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (Duration::from_millis(100)..=Duration::from_secs(60)).contains(&self.interval),
            "监控轮询间隔须在 100..60000 ms 内"
        );
        validate_request_limits(self.request_timeout, self.max_response_bytes)?;
        ensure!(
            self.limit > 0 && self.limit <= self.max_limit && self.max_limit <= 10_000,
            "须满足 1 <= limit <= max-limit <= 10000"
        );
        ensure!(
            self.max_cycles != Some(0) && self.max_consecutive_errors != Some(0),
            "次数上限必须大于零"
        );
        ensure!(
            self.max_duration
                .is_none_or(|d| !d.is_zero() && d <= Duration::from_secs(7 * 86400)),
            "监控时长须大于零且不超过七天"
        );
        ensure!(
            self.state_file.is_none() || self.initial == InitialPolicy::Now,
            "--state-file 与 recent 初始策略不能同时使用"
        );
        Ok(())
    }
}

pub(crate) fn validate_request_limits(timeout: Duration, bytes: usize) -> Result<()> {
    ensure!(
        (Duration::from_millis(100)..=Duration::from_secs(60)).contains(&timeout),
        "请求超时须在 100..60000 ms 内"
    );
    ensure!(
        (1024..=64 * 1024 * 1024).contains(&bytes),
        "响应上限须在 1 KiB..64 MiB 内"
    );
    Ok(())
}

#[derive(Serialize)]
pub struct Event {
    pub kind: &'static str,
    pub runtime_id: String,
    pub cycle: u64,
    pub observed_at: String,
    pub data: Value,
}

pub(crate) fn event(
    context: &FixedRuntimeContext,
    cycle: u64,
    kind: &'static str,
    data: Value,
) -> Event {
    Event {
        kind,
        runtime_id: context.runtime_id().into(),
        cycle,
        observed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        data,
    }
}

#[derive(Default, Serialize)]
pub struct MonitorSummary {
    pub runtime_id: String,
    pub cycles: u64,
    pub messages_displayed: u64,
    pub error_cycles: u64,
    pub held_cycles: u64,
    pub cursor_sessions: usize,
    pub stop_reason: String,
    pub elapsed_ms: f64,
    pub coverage_proven: bool,
}

pub(crate) struct Batch {
    pub messages: Vec<Value>,
    pub next: HashMap<String, i64>,
    pub meta: Value,
    pub incomplete: bool,
    pub truncated: bool,
}

fn parse_batch(data: &Value, limit: usize) -> Result<Batch> {
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("消息响应缺少数组"))?;
    ensure!(
        messages.len() <= limit
            && data.get("count").and_then(Value::as_u64) == Some(messages.len() as u64),
        "消息响应数量不一致"
    );
    let next = state::parse_sessions(
        data.get("new_state")
            .ok_or_else(|| anyhow::anyhow!("消息响应缺少游标"))?,
    )?;
    for message in messages {
        let user = message
            .get("username")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("消息缺少会话身份"))?;
        let time = message
            .get("timestamp")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("消息时间无效"))?;
        ensure!(
            time >= 0 && next.get(user).is_some_and(|next| *next >= time),
            "消息与返回游标不一致"
        );
    }
    let meta = data.get("meta").cloned().unwrap_or(Value::Null);
    let incomplete = !metadata_allows_progress(&meta);
    let mut truncated = limit != 0 && messages.len() >= limit;
    for container in [data, &meta] {
        for key in ["truncated", "has_more"] {
            if let Some(value) = container.get(key) {
                truncated |= value
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("无效截断标志"))?;
            }
        }
    }
    Ok(Batch {
        messages: messages.clone(),
        next,
        meta,
        incomplete,
        truncated,
    })
}

pub(crate) fn metadata_allows_progress(meta: &Value) -> bool {
    matches!(
        meta.get("status").and_then(Value::as_str),
        Some("ok" | "windowed")
    ) && meta
        .get("unknown_shards")
        .and_then(Value::as_array)
        .is_some_and(|a| a.is_empty())
}

fn merge_cursor(
    previous: &HashMap<String, i64>,
    batch: &Batch,
    floor: i64,
) -> Result<HashMap<String, i64>> {
    let mut next = previous.clone();
    for (user, timestamp) in &batch.next {
        if let Some(old) = previous.get(user) {
            ensure!(timestamp >= old, "返回游标倒退，已保留原状态");
            next.insert(user.clone(), *timestamp);
        } else {
            let returned = batch
                .messages
                .iter()
                .any(|message| message["username"].as_str() == Some(user.as_str()));
            // daemon 对新会话且无返回消息时可能直接推进；显式补上起点，防止静默跳过。
            next.insert(
                user.clone(),
                if returned {
                    *timestamp
                } else {
                    (*timestamp).min(floor)
                },
            );
        }
    }
    ensure!(next.len() <= MAX_SESSIONS, "游标会话数量超过上限");
    state::validate_size(&next)?;
    Ok(next)
}

// Commit only after delivery succeeds, and never commit a provisional batch.
fn deliver_and_commit(
    cursor: &mut Option<HashMap<String, i64>>,
    next: HashMap<String, i64>,
    held: bool,
    deliver: impl FnOnce() -> Result<()>,
) -> Result<()> {
    deliver()?;
    if !held {
        *cursor = Some(next);
    }
    Ok(())
}

fn emitted_state(
    context: &FixedRuntimeContext,
    emit: bool,
    held: bool,
    current: Option<&HashMap<String, i64>>,
    next: &HashMap<String, i64>,
) -> Option<StateFile> {
    if !emit {
        return None;
    }
    // A provisional batch must never advertise an uncommitted cursor.
    let sessions = if held { current? } else { next };
    Some(StateFile::new(context, sessions.clone()))
}

/// sink 返回成功代表本轮输出已经写完；输出失败不推进游标。取消会释放当前管道。
pub async fn run_monitor(
    context: &FixedRuntimeContext,
    options: &MonitorOptions,
    cancel: &Cancellation,
    sink: &mut impl FnMut(Event) -> Result<()>,
) -> Result<MonitorSummary> {
    options.validate()?;
    let mut cursor = match &options.state_file {
        Some(path) => Some(state::load(context, path)?.sessions),
        None => None,
    };
    let started = Instant::now();
    let deadline = options.max_duration.map(|d| started + d);
    // recent 的回看起点只算一次，失败重试不让窗口随墙钟移动。
    let floor = chrono::Utc::now().timestamp().saturating_sub(86400);
    let mut summary = MonitorSummary {
        runtime_id: context.runtime_id().into(),
        ..Default::default()
    };
    let mut active_limit = options.limit;
    let mut consecutive_errors = 0;
    sink(event(
        context,
        0,
        "started",
        json!({
            "mode":"monitor", "initial":if cursor.is_some() {"state_file"} else if options.initial == InitialPolicy::Now {"now"} else {"recent_24h"},
            "interval_ms": options.interval.as_millis(), "limit":active_limit, "max_limit":options.max_limit,
            "state_file_access":"read_only", "daemon_lifecycle":"existing_only", "coverage_proven":false,
            "cursor_boundary":CURSOR_WARNING
        }),
    ))?;
    loop {
        if let Some(reason) = stop_reason(cancel, deadline) {
            summary.stop_reason = reason.into();
            break;
        }
        if options.max_cycles.is_some_and(|max| summary.cycles >= max) {
            summary.stop_reason = "cycle_limit".into();
            break;
        }
        summary.cycles += 1;
        let baseline = cursor.is_none();
        // None + limit=0 是当前 daemon 的全会话快照策略，仅建立基线，不显示历史。
        let request = Request::NewMessages {
            state: cursor.clone(),
            limit: if baseline { 0 } else { active_limit },
            with_meta: true,
            debug_source: options.debug_source,
        };
        let reply = transport::request(
            context,
            request,
            options.request_timeout,
            options.max_response_bytes,
            cancel,
            deadline,
        )
        .await;
        let result = reply.and_then(|reply| {
            parse_batch(&reply.data, if baseline { 0 } else { active_limit })
                .map(|batch| (batch, reply.timing))
                .map_err(|error| transport::QueryFailure::protocol(error.to_string()))
        });
        match result {
            Err(error) if error.is_stop() => {
                summary.stop_reason = error.code.into();
                break;
            }
            Err(error) => {
                summary.error_cycles += 1;
                consecutive_errors += 1;
                sink(event(
                    context,
                    summary.cycles,
                    "cycle_error",
                    json!({"error":error,"cursor_preserved":true,"consecutive_errors":consecutive_errors}),
                ))?;
            }
            Ok((batch, timing)) => {
                let merged = if baseline {
                    Ok(batch.next.clone())
                } else {
                    merge_cursor(cursor.as_ref().expect("已有基线"), &batch, floor)
                };
                match merged {
                    Err(error) => {
                        summary.error_cycles += 1;
                        consecutive_errors += 1;
                        sink(event(
                            context,
                            summary.cycles,
                            "cycle_error",
                            json!({"error":error.to_string(),"cursor_preserved":true,"consecutive_errors":consecutive_errors}),
                        ))?;
                    }
                    Ok(mut next) => {
                        consecutive_errors = 0;
                        let held = batch.incomplete || batch.truncated;
                        if held {
                            summary.held_cycles += 1;
                        }
                        if baseline && options.initial == InitialPolicy::Recent {
                            for timestamp in next.values_mut() {
                                *timestamp = floor;
                            }
                        }
                        let state = emitted_state(
                            context,
                            options.emit_state,
                            held,
                            cursor.as_ref(),
                            &next,
                        );
                        let count = batch.messages.len();
                        let mut data = json!({
                            "messages":batch.messages, "count":count, "limit":if baseline {0} else {active_limit},
                            "possible_truncation":batch.truncated, "source_incomplete":batch.incomplete,
                            "cursor_committed":!held, "delivery":if held {"provisional_may_repeat"} else {"timestamp_cursor_accepted"},
                            "coverage_proven":false, "cursor_stalled":!baseline && cursor.as_ref() == Some(&next),
                            "limit_ceiling_reached":batch.truncated && active_limit == options.max_limit,
                            "freshness_status":batch.meta.get("status"), "timing":timing,
                            "tracked_sessions":if held {cursor.as_ref().map_or(0, HashMap::len)} else {next.len()}
                        });
                        if options.with_meta || options.debug_source {
                            data["meta"] = batch.meta;
                        }
                        if let Some(state) = state {
                            data["state"] = serde_json::to_value(state)?;
                        }
                        deliver_and_commit(&mut cursor, next, held, || {
                            sink(event(
                                context,
                                summary.cycles,
                                if held {
                                    "cursor_held"
                                } else if baseline {
                                    "baseline"
                                } else if count == 0 {
                                    "heartbeat"
                                } else {
                                    "messages"
                                },
                                data,
                            ))
                        })?;
                        summary.messages_displayed += count as u64;
                        if batch.truncated && active_limit < options.max_limit {
                            active_limit = active_limit.saturating_mul(2).min(options.max_limit);
                            sink(event(
                                context,
                                summary.cycles,
                                "limit_increased",
                                json!({"limit":active_limit,"cursor_preserved":true}),
                            ))?;
                        }
                    }
                }
            }
        }
        if options
            .max_consecutive_errors
            .is_some_and(|max| consecutive_errors >= max)
        {
            summary.stop_reason = "error_limit".into();
            break;
        }
        // 每轮完成后至少休眠设定间隔，不追赶错过的 tick，不并发堆积请求。
        if let Some(reason) = wait_interval(options.interval, cancel, deadline).await {
            summary.stop_reason = reason.into();
            break;
        }
    }
    summary.cursor_sessions = cursor.as_ref().map_or(0, HashMap::len);
    summary.elapsed_ms = millis(started.elapsed());
    let mut stopped = serde_json::to_value(&summary)?;
    if options.emit_state {
        if let Some(cursor) = cursor {
            stopped["state"] = serde_json::to_value(StateFile::new(context, cursor))?;
        }
    }
    sink(event(context, summary.cycles, "stopped", stopped))?;
    Ok(summary)
}

pub(crate) fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
pub(crate) fn stop_reason(
    cancel: &Cancellation,
    deadline: Option<Instant>,
) -> Option<&'static str> {
    if cancel.is_cancelled() {
        Some("cancelled")
    } else if deadline.is_some_and(|d| Instant::now() >= d) {
        Some("duration_elapsed")
    } else {
        None
    }
}
pub(crate) async fn wait_interval(
    interval: Duration,
    cancel: &Cancellation,
    deadline: Option<Instant>,
) -> Option<&'static str> {
    if let Some(reason) = stop_reason(cancel, deadline) {
        return Some(reason);
    }
    let interval = deadline.map_or(interval, |d| {
        interval.min(d.saturating_duration_since(Instant::now()))
    });
    tokio::select! { biased; _ = cancel.cancelled() => Some("cancelled"), _ = tokio::time::sleep(interval) => stop_reason(cancel, deadline) }
}

#[cfg(test)]
mod cursor_recovery_tests {
    use super::*;

    fn sessions(timestamp: i64) -> HashMap<String, i64> {
        HashMap::from([("synthetic-session".into(), timestamp)])
    }

    fn response() -> Value {
        json!({
            "messages": [{"username": "synthetic-session", "timestamp": 20}],
            "count": 1,
            "new_state": {"synthetic-session": 20},
            "meta": {"status": "ok", "unknown_shards": []}
        })
    }

    #[test]
    fn failed_delivery_preserves_baseline_or_existing_cursor_until_retry() {
        for initial in [None, Some(sessions(10))] {
            let mut cursor = initial.clone();
            let mut deliveries = 0;
            let error = deliver_and_commit(&mut cursor, sessions(20), false, || {
                deliveries += 1;
                anyhow::bail!("synthetic sink failure")
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "synthetic sink failure");
            assert_eq!(deliveries, 1);
            assert_eq!(cursor, initial);

            deliver_and_commit(&mut cursor, sessions(20), false, || {
                deliveries += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(deliveries, 2);
            assert_eq!(cursor, Some(sessions(20)));
        }
    }

    #[test]
    fn incomplete_metadata_holds_repeated_batches_then_recovers() {
        for meta in [
            Value::Null,
            json!({"status": "partial", "unknown_shards": []}),
            json!({"status": "ok", "unknown_shards": ["synthetic-shard"]}),
            json!({"status": "ok"}),
            json!({"status": "ok", "unknown_shards": null}),
        ] {
            let mut data = response();
            data["meta"] = meta;
            let batch = parse_batch(&data, 2).unwrap();
            assert!(batch.incomplete);
            assert!(!batch.truncated);
            for initial in [None, Some(sessions(10))] {
                let mut cursor = initial.clone();
                let mut deliveries = 0;
                for _ in 0..2 {
                    deliver_and_commit(
                        &mut cursor,
                        batch.next.clone(),
                        batch.incomplete || batch.truncated,
                        || {
                            deliveries += 1;
                            Ok(())
                        },
                    )
                    .unwrap();
                    assert_eq!(cursor, initial);
                }
                assert_eq!(deliveries, 2);
                for status in ["ok", "windowed"] {
                    let mut recovered = response();
                    recovered["meta"]["status"] = json!(status);
                    let batch = parse_batch(&recovered, 2).unwrap();
                    assert!(!batch.incomplete && !batch.truncated);
                    deliver_and_commit(
                        &mut cursor,
                        batch.next,
                        batch.incomplete || batch.truncated,
                        || Ok(()),
                    )
                    .unwrap();
                    assert_eq!(cursor, Some(sessions(20)));
                }
            }
        }
    }

    #[test]
    fn incomplete_empty_baseline_remains_uninitialized_until_complete_snapshot() {
        let mut data = response();
        data["messages"] = json!([]);
        data["count"] = json!(0);
        data["meta"]["status"] = json!("partial");
        let mut cursor = None;
        for status in ["partial", "ok"] {
            data["meta"]["status"] = json!(status);
            let batch = parse_batch(&data, 0).unwrap();
            assert!(!batch.truncated);
            assert_eq!(batch.incomplete, status == "partial");
            deliver_and_commit(
                &mut cursor,
                batch.next,
                batch.incomplete || batch.truncated,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(cursor, (status == "ok").then(|| sessions(20)));
        }
    }

    #[test]
    fn truncation_holds_cursor_until_larger_complete_batch() {
        let mut cases = vec![(response(), 1)];
        for key in ["truncated", "has_more"] {
            let mut top_level = response();
            top_level[key] = json!(true);
            cases.push((top_level, 2));
            let mut metadata = response();
            metadata["meta"][key] = json!(true);
            cases.push((metadata, 2));
        }
        for (data, limit) in cases {
            let batch = parse_batch(&data, limit).unwrap();
            assert!(batch.truncated);
            assert!(!batch.incomplete);
            let mut cursor = Some(sessions(10));
            deliver_and_commit(
                &mut cursor,
                batch.next,
                batch.incomplete || batch.truncated,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(cursor, Some(sessions(10)));

            let batch = parse_batch(&response(), 2).unwrap();
            let next = merge_cursor(cursor.as_ref().unwrap(), &batch, 0).unwrap();
            deliver_and_commit(
                &mut cursor,
                next,
                batch.incomplete || batch.truncated,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(cursor, Some(sessions(20)));
        }
    }

    #[test]
    fn malformed_or_regressing_batches_are_rejected_before_recovery() {
        let previous = sessions(10);
        for (key, value) in [
            ("messages", Value::Null),
            ("count", json!(2)),
            ("new_state", json!({"synthetic-session": 19})),
            ("has_more", json!("true")),
        ] {
            let mut invalid = response();
            invalid[key] = value;
            assert!(parse_batch(&invalid, 2).is_err(), "{key}");
        }

        let mut regressing = response();
        regressing["messages"][0]["timestamp"] = json!(9);
        regressing["new_state"]["synthetic-session"] = json!(9);
        let batch = parse_batch(&regressing, 2).unwrap();
        assert!(merge_cursor(&previous, &batch, 0).is_err());
        assert_eq!(previous, sessions(10));

        let batch = parse_batch(&response(), 2).unwrap();
        let next = merge_cursor(&previous, &batch, 0).unwrap();
        let mut cursor = Some(previous);
        deliver_and_commit(&mut cursor, next, false, || Ok(())).unwrap();
        assert_eq!(cursor, Some(sessions(20)));
    }
}
