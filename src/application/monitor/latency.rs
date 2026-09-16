//! 沿用 legacy 的 DB/WAL mtime 变化观测，但不自行解密或修改 SQLite/WAL。
//! IPC 与后台实际解密/固定只读查询分别用单调时钟计时；不能据此证明网络延迟或消息因果链。
pub use super::statistics::RequestStatistics;
use super::statistics::Statistics;
use super::{
    event, metadata_allows_progress, millis, state, stop_reason, transport,
    validate_request_limits, wait_interval, Cancellation, Event, FixedRuntimeContext,
    RequestTiming,
};
use crate::ipc::Request;
use crate::service::protocol::monitor::{LatencyProbe, LATENCY_PROBE_KIND};
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};

pub struct LatencyOptions {
    pub duration: Duration,
    pub poll_interval: Duration,
    pub probe_interval: Duration,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
    pub sessions_limit: usize,
    pub history_limit: usize,
    pub chat: Option<String>,
    pub ipc_only: bool,
    pub debug_source: bool,
}
impl Default for LatencyOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(60),
            poll_interval: Duration::from_millis(30),
            probe_interval: Duration::from_secs(1),
            request_timeout: Duration::from_secs(20),
            max_response_bytes: 8 * 1024 * 1024,
            sessions_limit: 200,
            history_limit: 20,
            chat: None,
            ipc_only: false,
            debug_source: false,
        }
    }
}
impl LatencyOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (Duration::from_millis(100)..=Duration::from_secs(86400)).contains(&self.duration),
            "延迟观测时长须在 100 ms..24 h 内"
        );
        ensure!(
            (Duration::from_millis(30)..=Duration::from_secs(60)).contains(&self.poll_interval),
            "文件轮询间隔须在 30..60000 ms 内"
        );
        ensure!(
            (Duration::from_millis(200)..=Duration::from_secs(60)).contains(&self.probe_interval),
            "IPC 采样间隔须在 200..60000 ms 内"
        );
        validate_request_limits(self.request_timeout, self.max_response_bytes)?;
        ensure!(
            (1..=10_000).contains(&self.sessions_limit)
                && (1..=10_000).contains(&self.history_limit),
            "会话和历史 limit 须在 1..10000 内"
        );
        ensure!(
            self.chat
                .as_ref()
                .is_none_or(|chat| !chat.trim().is_empty() && chat.len() <= 512),
            "chat 参数无效"
        );
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
struct Stamp {
    exists: bool,
    size_bytes: u64,
    modified_unix_ns: Option<u128>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct SourceStamps {
    database: Stamp,
    wal: Stamp,
}

fn stamp(path: &Path, optional: bool) -> Result<Stamp> {
    state::checked_parent(path)?;
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Stamp {
                exists: false,
                size_bytes: 0,
                modified_unix_ns: None,
            })
        }
        Err(_) => anyhow::bail!("无法读取当前账号 session DB/WAL 元数据"),
    };
    ensure!(
        meta.is_file() && !state::is_reparse(&meta),
        "session DB/WAL 必须为普通文件"
    );
    let modified = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("文件 mtime 超出支持范围"))?;
    Ok(Stamp {
        exists: true,
        size_bytes: meta.len(),
        modified_unix_ns: Some(modified.as_nanos()),
    })
}
fn source_stamps(context: &FixedRuntimeContext) -> Result<SourceStamps> {
    let source = context
        .runtime
        .config
        .db_dir
        .join("session")
        .join("session.db");
    let mut wal = source.as_os_str().to_os_string();
    wal.push("-wal");
    Ok(SourceStamps {
        database: stamp(&source, false)?,
        wal: stamp(Path::new(&wal), true)?,
    })
}

#[derive(Serialize)]
struct PhaseSample {
    timing: RequestTiming,
    returned_count: usize,
    limit: usize,
    possible_truncation: bool,
    meta: Value,
}

fn phase_sample(
    reply: transport::Reply,
    rows: &str,
    limit: usize,
) -> std::result::Result<(PhaseSample, Value), transport::QueryFailure> {
    let Some(array) = reply.data.get(rows).and_then(Value::as_array) else {
        return Err(transport::QueryFailure::protocol(
            "查询响应缺少结果数组".into(),
        ));
    };
    if array.len() > limit
        || reply
            .data
            .get("count")
            .is_some_and(|count| count.as_u64() != Some(array.len() as u64))
    {
        return Err(transport::QueryFailure::protocol(
            "查询结果数量不一致".into(),
        ));
    }
    let sample = PhaseSample {
        returned_count: array.len(),
        limit,
        possible_truncation: array.len() == limit,
        timing: reply.timing,
        meta: reply.data.get("meta").cloned().unwrap_or(Value::Null),
    };
    Ok((sample, reply.data))
}

fn daemon_phase_sample(
    reply: transport::Reply,
    limit: usize,
) -> std::result::Result<(LatencyProbe, RequestTiming), transport::QueryFailure> {
    let probe: LatencyProbe = serde_json::from_value(reply.data)
        .map_err(|_| transport::QueryFailure::protocol("后台阶段探测响应缺失或格式无效".into()))?;
    probe
        .validate(limit)
        .map_err(|error| transport::QueryFailure::protocol(error.to_string()))?;
    Ok((probe, reply.timing))
}

#[derive(Default, Serialize)]
pub struct LatencySummary {
    pub runtime_id: String,
    pub polls: u64,
    pub successful_samples: u64,
    pub interrupted_samples: u64,
    pub source_incomplete_samples: u64,
    pub possible_truncation_samples: u64,
    pub metadata_unavailable_samples: u64,
    pub query_errors: u64,
    pub metadata_errors: u64,
    pub file_change_observations: u64,
    pub pending_file_change: bool,
    pub stop_reason: String,
    pub elapsed_ms: f64,
    pub network_latency_measured: bool,
    pub coverage_proven: bool,
    pub requests: BTreeMap<&'static str, RequestStatistics>,
}

struct SessionProgress {
    candidate: HashMap<String, i64>,
    advances: usize,
}

fn session_progress(
    data: &Value,
    previous: &HashMap<String, i64>,
    max_timestamp: i64,
) -> std::result::Result<SessionProgress, transport::QueryFailure> {
    let mut candidate = previous.clone();
    let mut advances = 0;
    for row in data["sessions"]
        .as_array()
        .expect("phase_sample 已检查结果数组")
    {
        let (Some(user), Some(timestamp)) = (row["username"].as_str(), row["timestamp"].as_i64())
        else {
            return Err(transport::QueryFailure::protocol(
                "会话身份/时间无效".into(),
            ));
        };
        state::validate_entry(user, timestamp, max_timestamp)
            .map_err(|_| transport::QueryFailure::protocol("会话身份/时间超出范围".into()))?;
        if previous.get(user).is_some_and(|old| timestamp > *old) {
            advances += 1;
        }
        candidate
            .entry(user.into())
            .and_modify(|old| *old = (*old).max(timestamp))
            .or_insert(timestamp);
    }
    if candidate.len() > super::MAX_SESSIONS {
        return Err(transport::QueryFailure::protocol("会话数量超过上限".into()));
    }
    Ok(SessionProgress {
        candidate,
        advances,
    })
}

/// 客户端只 stat 两个固定路径并发送 IPC；后台通过现有缓存路径解密，再执行固定只读探测。
#[derive(Default)]
struct SourceObserver {
    observed: Option<SourceStamps>,
    pending_since: Option<Instant>,
}

impl SourceObserver {
    fn poll(
        &mut self,
        context: &FixedRuntimeContext,
        summary: &mut LatencySummary,
        ipc_only: bool,
        poll_gap_ms: f64,
        sink: &mut impl FnMut(Event) -> Result<()>,
    ) -> Result<(Option<SourceStamps>, Option<f64>)> {
        if ipc_only {
            return Ok((None, None));
        }
        let started = Instant::now();
        let result = source_stamps(context);
        let stat_ms = Some(millis(started.elapsed()));
        let current = match result {
            Ok(stamps) => {
                if self.observed.as_ref().is_some_and(|old| old != &stamps) {
                    summary.file_change_observations += 1;
                    self.pending_since.get_or_insert_with(Instant::now);
                    sink(event(
                        context,
                        summary.polls,
                        "file_change",
                        json!({
                            "database_changed":self.observed.as_ref().is_some_and(|old| old.database != stamps.database),
                            "wal_changed":self.observed.as_ref().is_some_and(|old| old.wal != stamps.wal),
                            "observed_poll_gap_ms":poll_gap_ms, "source":stamps
                        }),
                    ))?;
                }
                self.observed = Some(stamps.clone());
                Some(stamps)
            }
            Err(error) => {
                summary.metadata_errors += 1;
                sink(event(
                    context,
                    summary.polls,
                    "metadata_error",
                    json!({"error":error.to_string(),"previous_observation_preserved":true}),
                ))?;
                None
            }
        };
        Ok((current, stat_ms))
    }
}

pub async fn run_latency(
    context: &FixedRuntimeContext,
    options: &LatencyOptions,
    cancel: &Cancellation,
    sink: &mut impl FnMut(Event) -> Result<()>,
) -> Result<LatencySummary> {
    options.validate()?;
    let started = Instant::now();
    let deadline = Some(started + options.duration);
    let mut summary = LatencySummary {
        runtime_id: context.runtime_id().into(),
        ..Default::default()
    };
    let mut statistics = Statistics::default();
    let mut observer = SourceObserver::default();
    let mut acknowledged: Option<SourceStamps> = None;
    let mut last_probe: Option<Instant> = None;
    let mut last_poll = Instant::now();
    let mut sessions_seen: HashMap<String, i64> = HashMap::new();
    sink(event(
        context,
        0,
        "started",
        json!({
            "mode":"latency", "duration_ms":options.duration.as_millis(), "poll_interval_ms":options.poll_interval.as_millis(),
            "probe_interval_ms":options.probe_interval.as_millis(), "source_observation":if options.ipc_only {"disabled"} else {"session_db_and_wal_mtime_and_size_only"},
            "daemon_lifecycle":"existing_only", "network_latency_ms":null, "daemon_decrypt_ms":null, "daemon_query_ms":null,
            "unavailable_reason":"daemon_phase_probe_not_completed_yet",
            "daemon_phase_query_kind":LATENCY_PROBE_KIND,
            "daemon_phase_boundary":"actual_crypto_workers_and_fixed_readonly_session_timestamp_query;not_sessions_or_history_rendering",
            "measurement_boundary":"local_observation_and_ipc_roundtrip_not_message_causality",
            "mtime_boundary":"fixed_size_wal_can_change_mtime;unchanged_mtime_does_not_prove_no_write",
            "summary_statistics":"successful_measured_client_and_daemon_phase_min_mean_max;client_total_p50_p95_last_1024_successful_requests;skipped_crypto_not_zero_filled"
        }),
    ))?;
    loop {
        if let Some(reason) = stop_reason(cancel, deadline) {
            summary.stop_reason = reason.into();
            break;
        }
        summary.polls += 1;
        let poll_gap_ms = millis(last_poll.elapsed());
        last_poll = Instant::now();
        let (current, stat_ms) =
            observer.poll(context, &mut summary, options.ipc_only, poll_gap_ms, sink)?;
        let changed = current
            .as_ref()
            .is_some_and(|value| acknowledged.as_ref().is_some_and(|old| old != value));
        let due = last_probe.is_none_or(|last| last.elapsed() >= options.probe_interval);
        // 文件变化可触发提前探测，但每次探测之间至少留 200 ms，避免失效后台被密集重试。
        let change_due =
            changed && last_probe.is_none_or(|last| last.elapsed() >= Duration::from_millis(200));
        if due || change_due {
            last_probe = Some(Instant::now());
            let probe_started = Instant::now();
            let mut phases = serde_json::Map::new();
            let mut failure = None;
            let mut active_operation = "ping";
            let mut daemon_sample = None;
            let mut session_sample = None;
            let mut history_sample = None;
            let mut pending_progress = None;
            match transport::request(
                context,
                Request::Ping,
                options.request_timeout,
                1024,
                cancel,
                deadline,
            )
            .await
            {
                Ok(reply) if reply.data.get("pong").and_then(Value::as_bool) == Some(true) => {
                    statistics.success("ping", &reply.timing);
                    phases.insert("ping".into(), serde_json::to_value(reply.timing)?);
                }
                Ok(_) => failure = Some(transport::QueryFailure::protocol("无效 Pong".into())),
                Err(error) => failure = Some(error),
            }
            if failure.is_none() {
                // 在 Sessions/History 之前探测，避免普通查询先把待测冷缓存变成命中。
                active_operation = "daemon_probe";
                let request = Request::LatencyProbe {
                    limit: options.sessions_limit,
                };
                match transport::request(
                    context,
                    request,
                    options.request_timeout,
                    options.max_response_bytes,
                    cancel,
                    deadline,
                )
                .await
                .and_then(|reply| daemon_phase_sample(reply, options.sessions_limit))
                {
                    Ok((probe, timing)) => {
                        statistics.success_probe(&timing, &probe);
                        phases.insert(
                            "daemon_probe".into(),
                            json!({"timing":timing,"measurement":&probe}),
                        );
                        daemon_sample = Some(probe);
                    }
                    Err(error) => failure = Some(error),
                }
            }
            if failure.is_none() {
                active_operation = "sessions";
                let request = Request::Sessions {
                    limit: options.sessions_limit,
                    with_meta: true,
                    debug_source: options.debug_source,
                };
                match transport::request(
                    context,
                    request,
                    options.request_timeout,
                    options.max_response_bytes,
                    cancel,
                    deadline,
                )
                .await
                .and_then(|reply| phase_sample(reply, "sessions", options.sessions_limit))
                {
                    Ok((sample, data)) => {
                        let progress = session_progress(
                            &data,
                            &sessions_seen,
                            chrono::Utc::now().timestamp().saturating_add(86400),
                        );
                        match progress {
                            Ok(progress) => {
                                statistics.success("sessions", &sample.timing);
                                pending_progress = Some(progress);
                            }
                            Err(error) => failure = Some(error),
                        }
                        phases.insert("sessions".into(), serde_json::to_value(&sample)?);
                        session_sample = Some(sample);
                    }
                    Err(error) => failure = Some(error),
                }
            }
            if failure.is_none() {
                if let Some(chat) = &options.chat {
                    active_operation = "history";
                    let request = Request::History {
                        chat: chat.clone(),
                        limit: options.history_limit,
                        offset: 0,
                        since: None,
                        until: None,
                        msg_type: None,
                        msg_types: None,
                        oldest_first: false,
                        with_meta: true,
                        debug_source: options.debug_source,
                    };
                    match transport::request(
                        context,
                        request,
                        options.request_timeout,
                        options.max_response_bytes,
                        cancel,
                        deadline,
                    )
                    .await
                    .and_then(|reply| phase_sample(reply, "messages", options.history_limit))
                    {
                        Ok((sample, _)) => {
                            statistics.success("history", &sample.timing);
                            phases.insert("history".into(), serde_json::to_value(&sample)?);
                            history_sample = Some(sample);
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            if let Some(error) = failure {
                statistics.unsuccessful(active_operation, &error);
                if error.is_stop() {
                    summary.interrupted_samples += 1;
                    summary.stop_reason = error.code.into();
                    sink(event(
                        context,
                        summary.polls,
                        "latency_interrupted",
                        json!({"operation":active_operation,"error":error,"completed_phases":phases,"observation_preserved":true}),
                    ))?;
                    break;
                }
                summary.query_errors += 1;
                sink(event(
                    context,
                    summary.polls,
                    "latency_error",
                    json!({"operation":active_operation,"error":error,"completed_phases":phases,"observation_preserved":true}),
                ))?;
            } else {
                summary.successful_samples += 1;
                let source_incomplete = session_sample
                    .as_ref()
                    .is_none_or(|sample| !metadata_allows_progress(&sample.meta))
                    || history_sample
                        .as_ref()
                        .is_some_and(|sample| !metadata_allows_progress(&sample.meta));
                let possible_truncation = session_sample
                    .as_ref()
                    .is_some_and(|sample| sample.possible_truncation)
                    || history_sample
                        .as_ref()
                        .is_some_and(|sample| sample.possible_truncation);
                if source_incomplete {
                    summary.source_incomplete_samples += 1;
                }
                if possible_truncation {
                    summary.possible_truncation_samples += 1;
                }
                if !options.ipc_only && current.is_none() {
                    summary.metadata_unavailable_samples += 1;
                }
                let gap = history_sample.as_ref().and_then(|sample| {
                    sample.meta["session_last_timestamp"]
                        .as_i64()?
                        .checked_sub(sample.meta["chat_latest_timestamp"].as_i64()?)
                });
                let daemon = daemon_sample.as_ref().expect("成功采样已验证后台阶段探测");
                sink(event(
                    context,
                    summary.polls,
                    "latency_sample",
                    json!({
                        "phases":phases, "metadata_stat_ms":stat_ms, "observed_poll_gap_ms":poll_gap_ms,
                        "probe_total_ms":millis(probe_started.elapsed()), "file_observation_to_probe_completion_ms":observer.pending_since.map(|time| millis(time.elapsed())),
                        "session_timestamp_advances":pending_progress.as_ref().expect("成功采样已验证会话进度").advances, "session_history_timestamp_gap_seconds":gap,
                        "timestamp_gap_semantics":"freshness_comparison_not_network_delay",
                        "source_incomplete":source_incomplete, "possible_truncation":possible_truncation,
                        "file_metadata_requested":!options.ipc_only,
                        "file_metadata_available":!options.ipc_only && current.is_some(),
                        "coverage_proven":false, "network_latency_ms":null,
                        "daemon_phase_query_kind":daemon.query_kind, "daemon_cache_mode":daemon.cache_mode,
                        "daemon_cache_resolve_ms":daemon.daemon_cache_resolve_ms,
                        "daemon_decrypt_ms":daemon.daemon_decrypt_ms,
                        "daemon_db_decrypt_ms":daemon.daemon_db_decrypt_ms,
                        "daemon_wal_apply_ms":daemon.daemon_wal_apply_ms,
                        "daemon_query_ms":daemon.daemon_query_ms,
                        "daemon_decrypt_skipped_reason":daemon.decrypt_skipped_reason
                    }),
                ))?;
                if !source_incomplete {
                    sessions_seen = pending_progress.expect("成功采样已验证会话进度").candidate;
                    if let Some(current) = current {
                        acknowledged = Some(current);
                        observer.pending_since = None;
                    }
                }
            }
        }
        if let Some(reason) = wait_interval(options.poll_interval, cancel, deadline).await {
            summary.stop_reason = reason.into();
            break;
        }
    }
    summary.pending_file_change = observer.pending_since.is_some();
    summary.elapsed_ms = millis(started.elapsed());
    summary.requests = statistics.finish();
    sink(event(
        context,
        summary.polls,
        "stopped",
        serde_json::to_value(&summary)?,
    ))?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_json() -> Value {
        json!({
            "version":1, "query_kind":LATENCY_PROBE_KIND, "cache_mode":"full_decrypt",
            "daemon_cache_resolve_ms":6.0, "daemon_decrypt_ms":3.0,
            "daemon_db_decrypt_ms":2.0, "daemon_wal_apply_ms":1.0, "daemon_query_ms":4.0,
            "decrypt_skipped_reason":null, "rows_read":2, "row_limit":10,
            "possible_truncation":false, "latest_timestamp":200
        })
    }

    fn reply(data: Value) -> transport::Reply {
        transport::Reply {
            data,
            timing: RequestTiming {
                total_ms: 12.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn session_progress_validates_and_merges_without_mutating_previous_state() {
        let previous = HashMap::from([("alice".into(), 100), ("bob".into(), 200)]);
        let data = json!({"sessions":[
            {"username":"alice","timestamp":120},
            {"username":"alice","timestamp":110},
            {"username":"bob","timestamp":190},
            {"username":"carol","timestamp":150}
        ]});
        let Ok(progress) = session_progress(&data, &previous, 1_000) else {
            panic!("synthetic session progress should be valid")
        };
        assert_eq!(progress.advances, 2);
        assert_eq!(progress.candidate["alice"], 120);
        assert_eq!(progress.candidate["bob"], 200);
        assert_eq!(progress.candidate["carol"], 150);
        assert_eq!(
            previous,
            HashMap::from([("alice".into(), 100), ("bob".into(), 200)])
        );
    }

    #[test]
    fn session_progress_rejects_invalid_rows_future_times_and_excess_sessions() {
        let previous = HashMap::from([("kept".into(), 100)]);
        for data in [
            json!({"sessions":[{"timestamp":100}]}),
            json!({"sessions":[{"username":"alice","timestamp":"100"}]}),
            json!({"sessions":[{"username":"alice","timestamp":1_001}]}),
        ] {
            assert!(session_progress(&data, &previous, 1_000).is_err());
            assert_eq!(previous, HashMap::from([("kept".into(), 100)]));
        }

        let full = (0..super::super::MAX_SESSIONS)
            .map(|index| (format!("user-{index}"), 100))
            .collect();
        assert!(session_progress(
            &json!({"sessions":[{"username":"overflow","timestamp":100}]}),
            &full,
            1_000,
        )
        .is_err());
    }

    #[test]
    fn daemon_probe_keeps_internal_measurements_separate_from_roundtrip() {
        let (cold, client) = daemon_phase_sample(reply(probe_json()), 10)
            .unwrap_or_else(|_| panic!("合成响应应通过校验"));
        assert_eq!(client.total_ms, 12.0);
        assert_eq!(cold.daemon_decrypt_ms, Some(3.0));
        assert_eq!(cold.daemon_query_ms, 4.0);
        let mut warm = probe_json();
        warm["cache_mode"] = json!("cache_hit");
        warm["daemon_decrypt_ms"] = Value::Null;
        warm["daemon_db_decrypt_ms"] = Value::Null;
        warm["daemon_wal_apply_ms"] = Value::Null;
        warm["decrypt_skipped_reason"] = json!("cache_hit_no_decryption");
        let (warm, _) = daemon_phase_sample(reply(warm), 10)
            .unwrap_or_else(|_| panic!("缓存命中响应应通过校验"));
        assert!(warm.daemon_decrypt_ms.is_none());
        assert_eq!(warm.daemon_query_ms, 4.0);
    }

    #[test]
    fn daemon_probe_rejects_missing_invalid_or_contradictory_measurements() {
        let mut missing = probe_json();
        missing.as_object_mut().unwrap().remove("daemon_query_ms");
        assert!(daemon_phase_sample(reply(missing), 10).is_err());
        for (field, value) in [
            ("version", json!(2)),
            ("query_kind", json!("unrecognized_query")),
            ("daemon_query_ms", json!(-1.0)),
            ("daemon_decrypt_ms", json!(99.0)),
            ("daemon_cache_resolve_ms", json!(1.0)),
            ("daemon_db_decrypt_ms", Value::Null),
            ("cache_mode", json!("cache_hit")),
            ("row_limit", json!(11)),
            ("rows_read", json!(11)),
            ("latest_timestamp", Value::Null),
            ("possible_truncation", json!(true)),
            ("decrypt_skipped_reason", json!("cache_hit_no_decryption")),
        ] {
            let mut invalid = probe_json();
            invalid[field] = value;
            assert!(
                daemon_phase_sample(reply(invalid), 10).is_err(),
                "应拒绝无效字段：{field}"
            );
        }
        let mut nonfinite: LatencyProbe = serde_json::from_value(probe_json()).unwrap();
        nonfinite.daemon_query_ms = f64::NAN;
        assert!(nonfinite.validate(10).is_err());
        nonfinite.daemon_query_ms = f64::INFINITY;
        assert!(nonfinite.validate(10).is_err());
    }

    #[test]
    fn measured_zero_is_valid_but_skipped_crypto_cannot_be_zero_filled() {
        let mut probe: LatencyProbe = serde_json::from_value(probe_json()).unwrap();
        probe.daemon_cache_resolve_ms = 0.0;
        probe.daemon_decrypt_ms = Some(0.0);
        probe.daemon_db_decrypt_ms = Some(0.0);
        probe.daemon_wal_apply_ms = None;
        probe.daemon_query_ms = 0.0;
        probe.validate(10).unwrap();
        probe.cache_mode = "cache_hit".into();
        probe.decrypt_skipped_reason = Some("cache_hit_no_decryption".into());
        assert!(probe.validate(10).is_err());
    }
}
