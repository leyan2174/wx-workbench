//! 只上传完整游标，不拆分查询。失败时外层 request 统一取消服务器暂存。
use super::*;
use crate::service::protocol::{monitor as wire, Call};
use std::{
    collections::{hash_map::IntoIter, HashMap},
    iter::Peekable,
    sync::Mutex,
};

fn next_chunk(entries: &mut Peekable<IntoIter<String, i64>>) -> Vec<(String, i64)> {
    let mut chunk = Vec::new();
    let mut bytes = 2;
    while let Some((name, timestamp)) = entries.peek() {
        let next = wire::entry_bytes(name, *timestamp) + 2 + usize::from(!chunk.is_empty());
        if chunk.len() == wire::MAX_CHUNK_ENTRIES || bytes + next > wire::CHUNK_BYTES {
            break;
        }
        bytes += next;
        chunk.push(entries.next().unwrap());
    }
    chunk
}

fn validate(sessions: &HashMap<String, i64>) -> std::result::Result<usize, QueryFailure> {
    let latest = chrono::Utc::now().timestamp().saturating_add(86400);
    if sessions.len() > wire::MAX_SESSIONS {
        return Err(QueryFailure::protocol("monitor_state_limit".into()));
    }
    let mut bytes = 2 + sessions.len().saturating_sub(1);
    for (name, timestamp) in sessions {
        if !wire::valid_entry(name, *timestamp, latest) {
            return Err(QueryFailure::protocol("monitor_entry_invalid".into()));
        }
        bytes += wire::entry_bytes(name, *timestamp);
        if bytes > wire::MAX_STATE_BYTES {
            return Err(QueryFailure::protocol("monitor_state_limit".into()));
        }
    }
    Ok(bytes)
}

async fn call(
    context: &FixedRuntimeContext,
    request: wire::Call,
    started: Instant,
) -> std::result::Result<Value, QueryFailure> {
    crate::service::client::request_with_timeout(
        &context.runtime,
        Call::Monitor { request },
        Duration::from_secs(60),
    )
    .await
    .map_err(|error| {
        let code = match error
            .downcast_ref::<crate::service::protocol::ServiceError>()
            .map(|e| e.code.as_str())
        {
            Some("monitor_busy") => "monitor_busy",
            Some("monitor_expired") => "monitor_expired",
            Some("monitor_cancelled") => "monitor_cancelled",
            Some("monitor_response_limit") => "response_limit_exceeded",
            Some("monitor_state_limit") => "monitor_state_limit",
            Some("monitor_upload_incomplete") => "monitor_upload_incomplete",
            Some("monitor_upload_missing") => "monitor_upload_missing",
            Some("monitor_query_failed") => "daemon_request_failed",
            _ => "monitor_service_failed",
        };
        QueryFailure::new("authenticated_service", code, started)
    })
}

pub(super) async fn exchange(
    context: &FixedRuntimeContext,
    request: Request,
    max_bytes: usize,
    phase: &AtomicU8,
    upload_id: &Mutex<Option<String>>,
    started: Instant,
) -> std::result::Result<Reply, QueryFailure> {
    let Request::NewMessages {
        state: Some(sessions),
        limit,
        with_meta,
        debug_source,
    } = request
    else {
        return Err(QueryFailure::new("serialize", "request_too_large", started));
    };
    let bytes = validate(&sessions)?;
    if !(1..=10_000).contains(&limit) || !(1024..=wire::MAX_RESPONSE_BYTES).contains(&max_bytes) {
        return Err(QueryFailure::protocol("monitor_query_limit".into()));
    }
    let serialize_ms = millis(started.elapsed());
    phase.store(5, Ordering::Relaxed);
    let roundtrip = Instant::now();
    let begun = call(
        context,
        wire::Call::Begin {
            sessions: sessions.len(),
            bytes,
        },
        started,
    )
    .await?;
    let id = begun
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| QueryFailure::protocol("monitor_begin_invalid".into()))?
        .to_owned();
    *upload_id.lock().unwrap() = Some(id.clone());
    let mut entries = sessions.into_iter().peekable();
    let mut chunks = 0;
    while entries.peek().is_some() {
        let chunk = next_chunk(&mut entries);
        if chunk.is_empty() {
            return Err(QueryFailure::protocol("monitor_chunk_limit".into()));
        }
        let response = call(
            context,
            wire::Call::Chunk {
                id: id.clone(),
                sequence: chunks,
                entries: chunk,
            },
            started,
        )
        .await?;
        chunks += 1;
        if response.get("sequence").and_then(Value::as_u64) != Some(chunks as u64) {
            return Err(QueryFailure::protocol("monitor_sequence_invalid".into()));
        }
    }
    let mut result = call(
        context,
        wire::Call::Finish {
            id,
            chunks,
            limit,
            with_meta,
            debug_source,
            max_response_bytes: max_bytes,
        },
        started,
    )
    .await?;
    let response_bytes = result
        .get("response_bytes")
        .and_then(Value::as_u64)
        .filter(|&n| n <= max_bytes as u64)
        .ok_or_else(|| QueryFailure::protocol("monitor_response_limit".into()))?
        as usize;
    let data = result
        .get_mut("data")
        .filter(|v| v.is_object())
        .ok_or_else(|| QueryFailure::protocol("monitor_response_invalid".into()))?
        .take();
    *upload_id.lock().unwrap() = None;
    Ok(Reply {
        data,
        timing: RequestTiming {
            serialize_ms: Some(serialize_ms),
            authenticated_roundtrip_ms: Some(millis(roundtrip.elapsed())),
            total_ms: millis(started.elapsed()),
            response_bytes,
            ..Default::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_preserve_every_session_and_fit_authenticated_envelope() {
        let state: HashMap<_, _> = (0..2148)
            .map(|i| (format!("中文-\\\"-{i:06}-{}", "x".repeat(100)), i))
            .collect();
        assert_eq!(
            validate(&state).unwrap_or_else(|_| panic!("valid synthetic state")),
            serde_json::to_vec(&state).unwrap().len()
        );
        let mut iter = state.clone().into_iter().peekable();
        let mut restored = HashMap::new();
        let mut sequence = 0;
        while iter.peek().is_some() {
            let chunk = next_chunk(&mut iter);
            assert!(!chunk.is_empty() && chunk.len() <= wire::MAX_CHUNK_ENTRIES);
            assert!(serde_json::to_vec(&chunk).unwrap().len() <= wire::CHUNK_BYTES);
            let envelope = crate::service::protocol::Envelope {
                version: 1,
                runtime_id: "a".repeat(64),
                token: "b".repeat(64),
                request: Call::Monitor {
                    request: wire::Call::Chunk {
                        id: "c".repeat(64),
                        sequence,
                        entries: chunk.clone(),
                    },
                },
            };
            assert!(
                serde_json::to_vec(&envelope).unwrap().len()
                    <= crate::service::protocol::MAX_REQUEST_BYTES
            );
            restored.extend(chunk);
            sequence += 1;
        }
        assert!(sequence > 1);
        assert_eq!(restored, state);
    }

    #[test]
    fn invalid_or_oversized_state_is_rejected_before_begin() {
        for state in [
            HashMap::from([("x\n".into(), 1)]),
            HashMap::from([("x".into(), -1)]),
            HashMap::from([("x".repeat(513), 1)]),
            (0..100_001).map(|i| (i.to_string(), 1)).collect(),
            (0..20_000)
                .map(|i| (format!("{i:08}{}", "x".repeat(504)), 1))
                .collect(),
        ] {
            assert!(validate(&state).is_err());
        }
    }

    #[tokio::test]
    async fn authenticated_pipe_preserves_state_and_cleans_up_all_outcomes() {
        use crate::{
            daemon::monitor_service,
            service::{protocol::ServiceError, transport as rpc},
        };
        use serde_json::json;
        use sha2::{Digest, Sha256};
        use std::sync::{atomic::AtomicUsize, Arc};
        use windows::Win32::{
            Foundation::FILETIME,
            System::Threading::{GetCurrentProcess, GetProcessTimes},
        };

        for mode in [
            "success",
            "bad_chunk",
            "timeout",
            "cancel",
            "response_limit",
        ] {
            let root = tempfile::tempdir().unwrap();
            let runtime = RuntimeContext {
                config: crate::config::Config {
                    key_store: None,
                    db_dir: root.path().join("source"),
                    keys_file: root.path().join("keys.json"),
                    decrypted_dir: root.path().join("decrypted"),
                    wechat_process: String::new(),
                },
                config_path: root.path().join("config.json"),
                root: root.path().into(),
                directory: root.path().into(),
                id: format!(
                    "{:x}",
                    Sha256::digest(root.path().to_string_lossy().as_bytes())
                ),
            };
            let mut birth = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            unsafe {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut birth,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            }
            .unwrap();
            let created = (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime);
            std::fs::write(root.path().join("daemon.pid"), serde_json::to_vec(&json!({
                "pid":std::process::id(), "exe":std::env::current_exe().unwrap(), "created":created, "runtime_id":runtime.id,
            })).unwrap()).unwrap();
            let service = monitor_service::Service::new();
            let calls = Arc::new(AtomicUsize::new(0));
            let queries = Arc::new(AtomicUsize::new(0));
            let query_started = Arc::new(tokio::sync::Notify::new());
            let serving_service = service.clone();
            let serving_calls = calls.clone();
            let serving_queries = queries.clone();
            let serving_started = query_started.clone();
            let handler = Arc::new(move |call| {
                let service = serving_service.clone();
                let calls = serving_calls.clone();
                let queries = serving_queries.clone();
                let started = serving_started.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let Call::Monitor { request } = call else {
                        panic!("monitor call required")
                    };
                    if mode == "bad_chunk"
                        && matches!(&request, wire::Call::Chunk { sequence: 1, .. })
                    {
                        return Err(ServiceError::new(
                            "synthetic_chunk_failure",
                            "synthetic failure",
                        ));
                    }
                    service
                        .handle(request, |request| async move {
                            queries.fetch_add(1, Ordering::SeqCst);
                            started.notify_one();
                            if matches!(mode, "timeout" | "cancel") {
                                std::future::pending::<()>().await;
                            }
                            let Request::NewMessages {
                                state: Some(state),
                                limit,
                                ..
                            } = request
                            else {
                                panic!()
                            };
                            assert_eq!(limit, 5);
                            assert_eq!(state.len(), 2148);
                            Response::ok(json!({"new_state":state,"messages":[],"count":0}))
                        })
                        .await
                }
            });
            let (shutdown, stopped) = tokio::sync::watch::channel(false);
            let server = tokio::spawn(rpc::serve(runtime.clone(), handler, stopped));
            tokio::time::timeout(Duration::from_secs(3), async {
                while !root.path().join("service-token.key").exists() {
                    assert!(
                        !server.is_finished(),
                        "authenticated server stopped before readiness"
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let context = FixedRuntimeContext::from_runtime(runtime).unwrap();
            let state: HashMap<_, _> = (0..2148)
                .map(|i| (format!("synthetic-{i:06}-{}", "x".repeat(64)), i))
                .collect();
            let cancel = Cancellation::new();
            let cancellation_task = if mode == "cancel" {
                let cancel = cancel.clone();
                Some(tokio::spawn(async move {
                    query_started.notified().await;
                    cancel.cancel();
                }))
            } else {
                None
            };
            let result = super::super::request(
                &context,
                Request::NewMessages {
                    state: Some(state.clone()),
                    limit: 5,
                    with_meta: true,
                    debug_source: false,
                },
                if mode == "timeout" {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                },
                if mode == "response_limit" {
                    1024
                } else {
                    1024 * 1024
                },
                &cancel,
                None,
            )
            .await;
            if let Some(task) = cancellation_task {
                // 测试中的前置连接失败也必须能收尾，不能永远等待查询开始通知。
                if !task.is_finished() {
                    task.abort();
                }
                let _ = task.await;
            }
            let remaining = service.active_uploads();
            service.clear();
            shutdown.send_replace(true);
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(remaining, 0, "upload leak after {mode}");
            assert!(calls.load(Ordering::SeqCst) > 2);
            assert_eq!(
                queries.load(Ordering::SeqCst),
                usize::from(mode != "bad_chunk")
            );
            if mode == "success" {
                let reply = result.unwrap_or_else(|_| panic!("authenticated monitor failed"));
                assert_eq!(
                    reply.data["new_state"],
                    serde_json::to_value(&state).unwrap()
                );
                assert!(reply.timing.authenticated_roundtrip_ms.is_some());
                assert!(reply.timing.connect_ms.is_none());
            } else {
                assert!(
                    result.is_err(),
                    "{mode} must fail without advancing the cursor"
                );
            }
        }
    }
}
