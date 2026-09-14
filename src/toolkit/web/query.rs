//! 固定账号的查询与监测适配；在有限槽位内等待，不重放已经发出的请求。
use super::server_types::Shared;
use crate::ipc::{Request, Response};
use crate::service::web::Call;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

#[derive(Debug)]
struct QueryBusy;
impl std::fmt::Display for QueryBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("查询繁忙")
    }
}
impl std::error::Error for QueryBusy {}

async fn acquire_query(
    queries: Arc<Semaphore>,
    waiting: Arc<Semaphore>,
    timeout: Duration,
) -> Result<OwnedSemaphorePermit> {
    match queries.clone().try_acquire_owned() {
        Ok(permit) => return Ok(permit),
        Err(TryAcquireError::Closed) => return Err(QueryBusy.into()),
        Err(TryAcquireError::NoPermits) => {}
    }
    // 短暂高峰可以等待，但等待人数和时间也要有上限；取消会自动归还等待名额。
    let _waiting = waiting.try_acquire_owned().map_err(|_| QueryBusy)?;
    tokio::time::timeout(timeout, queries.acquire_owned())
        .await
        .map_err(|_| QueryBusy)?
        .map_err(|_| QueryBusy.into())
}

pub(super) fn is_busy(error: &anyhow::Error) -> bool {
    error.is::<QueryBusy>()
        || error
            .downcast_ref::<crate::service::protocol::ServiceError>()
            .is_some_and(|error| error.code == "busy")
}

pub async fn request(state: &Shared, request: Request) -> Result<Value> {
    match request {
        Request::History {
            chat,
            limit,
            offset,
            since,
            ..
        } => {
            web(
                state,
                Call::History {
                    chat,
                    limit,
                    offset,
                    since,
                },
            )
            .await
        }
        request => raw(state, request, 8 * 1024 * 1024).await,
    }
}

pub async fn web(state: &Shared, call: Call) -> Result<Value> {
    let _permit = acquire_query(
        state.queries.clone(),
        state.query_waiters.clone(),
        Duration::from_secs(2),
    )
    .await?;
    crate::service::client::request_with_timeout(
        &state.runtime,
        crate::service::protocol::Call::Web {
            request: Box::new(call),
        },
        Duration::from_secs(30),
    )
    .await
}

// 请求只发送一次；超时或断连不会重放可能产生写入的操作。
async fn raw(state: &Shared, request: Request, maximum: usize) -> Result<Value> {
    let _permit = acquire_query(
        state.queries.clone(),
        state.query_waiters.clone(),
        Duration::from_secs(2),
    )
    .await?;
    let timeout = if matches!(&request, Request::Ping) {
        1
    } else {
        20
    };
    let result = tokio::time::timeout(Duration::from_secs(timeout), async {
        use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced};
        let pipe = state.runtime.pipe_name();
        let name = pipe.to_ns_name::<GenericNamespaced>()?;
        let mut stream = interprocess::local_socket::tokio::Stream::connect(name).await?;
        stream
            .write_all((serde_json::to_string(&request)? + "\n").as_bytes())
            .await?;
        let mut reader = BufReader::new(stream).take(maximum as u64 + 1);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        ensure!(line.len() <= maximum, "查询响应超过限额");
        let response: Response = serde_json::from_str(&line)?;
        response.require_success()?;
        Ok::<_, anyhow::Error>(response.data)
    })
    .await?;
    result
}

pub async fn monitor(state: Arc<Shared>) {
    let mut stop = state.shutdown.subscribe();
    let opened = tokio::select! {
        _ = stop.changed() => return,
        result = web(&state, Call::MonitorOpen {}) => result,
    };
    let Ok(opened) = opened else {
        state.event("monitor_status", json!({"status":"error"}));
        return;
    };
    let (Some(mut cursor), Some(epoch)) = (opened["cursor"].as_u64(), opened["epoch"].as_str())
    else {
        state.event("monitor_status", json!({"status":"error"}));
        return;
    };
    let epoch = epoch.to_owned();
    let Some(session) = opened["session"]
        .as_str()
        .filter(|session| session.len() <= 256)
    else {
        state.event("monitor_status", json!({"status":"error"}));
        return;
    };
    state.records.lock().unwrap().monitor_session = Some(session.to_owned());
    loop {
        if *stop.borrow() {
            break;
        }
        let poll = async {
            let page = web(
                &state,
                Call::MonitorEvents {
                    epoch: epoch.clone(),
                    after: cursor,
                },
            )
            .await?;
            let next = page["cursor"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("missing cursor"))?;
            if page["reset"] == true {
                cursor = next;
                state.event("reset", json!({"reason":"lagged","reload":["history"]}));
                return Ok::<_, anyhow::Error>(());
            }
            ensure!(next >= cursor, "event cursor regressed");
            let events = page["events"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("missing events"))?;
            ensure!(events.len() <= 128, "event page too large");
            let mut previous = cursor;
            for event in events {
                let seq = event["seq"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("missing event sequence"))?;
                ensure!(seq > previous && seq <= next, "invalid event sequence");
                previous = seq;
            }
            for event in events {
                let name = match event["name"].as_str() {
                    Some("message") => "message",
                    Some("monitor_status") => "monitor_status",
                    Some("image_status") => "image_status",
                    _ => continue,
                };
                // SSE identifiers remain reserved for the task service's own sequence.
                state.event(name, event["data"].clone());
            }
            cursor = next;
            Ok(())
        };
        tokio::select! {
            _ = stop.changed() => break,
            result = poll => if result.is_err() {
                state.event("monitor_status", json!({"status":"error"}));
            }
        }
        tokio::select! {
            _ = stop.changed() => break,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
}

// The permit lives in the blocking closure even when its async caller times out.
static STARTUPS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

pub(super) async fn ensure_detached(runtime: crate::runtime::RuntimeContext) -> Result<()> {
    let permit = STARTUPS.clone().try_acquire_owned()?;
    tokio::time::timeout(
        Duration::from_secs(25),
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            crate::cli::transport::ensure_running(&runtime)
        }),
    )
    .await??
}

pub fn contacts(mut value: Value) -> Value {
    if let Some(rows) = value.get_mut("contacts").and_then(Value::as_array_mut) {
        for row in rows {
            row["name"] = row["display"].clone();
        }
    }
    value
}

pub fn sessions(mut value: Value) -> Value {
    if let Some(rows) = value.get_mut("sessions").and_then(Value::as_array_mut) {
        for row in rows {
            row["name"] = row["chat"].clone();
            row["last_ts"] = row["timestamp"].clone();
            row["type"] = row["chat_type"].clone();
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_until_queued(waiting: &Semaphore) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while waiting.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn query_queue_is_bounded_and_resumes_after_release() {
        let active = Arc::new(Semaphore::new(1));
        let waiting = Arc::new(Semaphore::new(1));
        let held = active.clone().acquire_owned().await.unwrap();
        let next = tokio::spawn(acquire_query(
            active.clone(),
            waiting.clone(),
            Duration::from_secs(5),
        ));
        wait_until_queued(&waiting).await;
        assert!(is_busy(
            &acquire_query(active.clone(), waiting.clone(), Duration::from_secs(5))
                .await
                .unwrap_err()
        ));
        assert_eq!(active.available_permits(), 0);
        drop(held);
        let permit = next.await.unwrap().unwrap();
        assert_eq!(waiting.available_permits(), 1);
        assert_eq!(active.available_permits(), 0);
        drop(permit);
        assert_eq!(active.available_permits(), 1);
    }

    #[tokio::test]
    async fn query_queue_timeout_releases_its_waiting_slot() {
        let active = Arc::new(Semaphore::new(0));
        let waiting = Arc::new(Semaphore::new(1));
        let error = acquire_query(active.clone(), waiting.clone(), Duration::from_millis(5))
            .await
            .unwrap_err();
        assert!(is_busy(&error));
        assert_eq!(waiting.available_permits(), 1);
        assert_eq!(active.available_permits(), 0);
    }

    #[tokio::test]
    async fn cancelled_query_waiter_does_not_take_a_later_permit() {
        let active = Arc::new(Semaphore::new(1));
        let waiting = Arc::new(Semaphore::new(1));
        let held = active.clone().acquire_owned().await.unwrap();
        let next = tokio::spawn(acquire_query(
            active.clone(),
            waiting.clone(),
            Duration::from_secs(5),
        ));
        wait_until_queued(&waiting).await;
        next.abort();
        assert!(next.await.unwrap_err().is_cancelled());
        assert_eq!(waiting.available_permits(), 1);
        drop(held);
        assert_eq!(active.available_permits(), 1);
    }

    #[test]
    fn busy_error_classification_preserves_nonbusy_errors() {
        assert!(is_busy(&QueryBusy.into()));
        assert!(is_busy(&anyhow::Error::new(QueryBusy).context("query")));
        assert!(!is_busy(&anyhow::anyhow!("查询繁忙")));
        assert!(!is_busy(&anyhow::anyhow!("synthetic disconnected pipe")));
        assert!(is_busy(
            &crate::service::protocol::ServiceError::new("busy", "busy").into()
        ));
        assert!(!is_busy(
            &crate::service::protocol::ServiceError::new("unauthorized", "busy").into()
        ));
    }
}
