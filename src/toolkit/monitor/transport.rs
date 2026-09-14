//! 复用现有 IPC 类型和账号管道命名，不调用会自动启动后台的 CLI transport。
use super::{millis, Cancellation};
#[cfg(test)]
use crate::ipc::Response;
use crate::{ipc::Request, runtime::RuntimeContext};
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::Value;
#[path = "chunked.rs"]
mod chunked;
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant},
};

/// 配置仅在构造时读取一次；私有字段禁止轮询中换账号或改管道身份。
pub struct FixedRuntimeContext {
    pub(super) runtime: RuntimeContext,
    pipe: String,
}
impl FixedRuntimeContext {
    pub fn load() -> Result<Self> {
        Self::from_runtime(RuntimeContext::load()?)
    }
    pub fn from_runtime(runtime: RuntimeContext) -> Result<Self> {
        ensure!(
            runtime.id.len() == 64 && runtime.id.bytes().all(|b| b.is_ascii_hexdigit()),
            "运行账号标识无效"
        );
        let pipe = runtime.pipe_name();
        Ok(Self { runtime, pipe })
    }
    pub fn runtime_id(&self) -> &str {
        &self.runtime.id
    }
}

#[derive(Default, Debug, Serialize)]
pub struct RequestTiming {
    pub serialize_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub write_ms: Option<f64>,
    /// 含后台排队、刷新/解密、查询、序列化及传输，不是纯查询耗时。
    pub wait_read_ms: Option<f64>,
    pub parse_ms: Option<f64>,
    /// 认证分块服务只报告完整 RPC 耗时，未测量的网络分项为 None。
    pub authenticated_roundtrip_ms: Option<f64>,
    pub total_ms: f64,
    pub response_bytes: usize,
}
pub(super) struct Reply {
    pub data: Value,
    pub timing: RequestTiming,
}

#[derive(Serialize)]
pub(super) struct QueryFailure {
    pub phase: &'static str,
    pub code: &'static str,
    pub detail: String,
    /// 只有持有实际起点的失败路径才填耗时；协议语义错误不伪造 0 ms。
    pub elapsed_ms: Option<f64>,
}
impl QueryFailure {
    fn new(phase: &'static str, code: &'static str, started: Instant) -> Self {
        Self {
            phase,
            code,
            detail: code.into(),
            elapsed_ms: Some(millis(started.elapsed())),
        }
    }
    pub fn protocol(detail: String) -> Self {
        Self {
            phase: "protocol",
            code: "invalid_response",
            detail,
            elapsed_ms: None,
        }
    }
    pub fn is_stop(&self) -> bool {
        matches!(self.code, "cancelled" | "duration_elapsed")
    }
}

pub(super) async fn request(
    context: &FixedRuntimeContext,
    request: Request,
    timeout: Duration,
    max_bytes: usize,
    cancel: &Cancellation,
    deadline: Option<Instant>,
) -> std::result::Result<Reply, QueryFailure> {
    let started = Instant::now();
    if cancel.is_cancelled() {
        return Err(QueryFailure::new("cancel", "cancelled", started));
    }
    let remaining = deadline.map_or(timeout, |deadline| {
        timeout.min(deadline.saturating_duration_since(started))
    });
    if remaining.is_zero() {
        return Err(QueryFailure::new("deadline", "duration_elapsed", started));
    }
    let budget_code = if remaining < timeout {
        "duration_elapsed"
    } else {
        "request_timeout"
    };
    let phase = AtomicU8::new(0);
    let upload_id = std::sync::Mutex::new(None);
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(QueryFailure::new(phase_name(&phase), "cancelled", started)),
        result = tokio::time::timeout(remaining, exchange(context, request, max_bytes, &phase, &upload_id)) => match result {
            Ok(result) => result,
            Err(_) => Err(QueryFailure::new(phase_name(&phase), budget_code, started)),
        }
    };
    // 清理独立于已取消的请求，最多额外等待 1 秒；丢失 Begin 响应则由服务 TTL 回收。
    let unfinished = upload_id.lock().unwrap().take();
    if let Some(id) = unfinished {
        let _ = crate::service::client::request_with_timeout(
            &context.runtime,
            crate::service::protocol::Call::Monitor {
                request: crate::service::protocol::monitor::Call::Abort { id },
            },
            Duration::from_secs(1),
        )
        .await;
    }
    result
}

#[cfg(windows)]
async fn exchange(
    context: &FixedRuntimeContext,
    request: Request,
    max_bytes: usize,
    current_phase: &AtomicU8,
    upload_id: &std::sync::Mutex<Option<String>>,
) -> std::result::Result<Reply, QueryFailure> {
    use crate::service::{query_client, transport::framing};
    let started = Instant::now();
    if context.pipe != context.runtime.pipe_name() {
        return Err(QueryFailure::new(
            "connect",
            "invalid_account_pipe",
            started,
        ));
    }
    let payload = serde_json::to_vec(&request)
        .map_err(|_| QueryFailure::new("serialize", "request_encoding_failed", started))?;
    if payload.len() + context.runtime.id.len() + 256 > crate::ipc::QUERY_REQUEST_LIMIT {
        drop(payload);
        return chunked::exchange(
            context,
            request,
            max_bytes,
            current_phase,
            upload_id,
            started,
        )
        .await;
    }
    drop(payload);
    let serialize_ms = millis(started.elapsed());
    current_phase.store(1, Ordering::Relaxed);
    let phase = Instant::now();
    let mut reader = query_client::connect_query(&context.runtime)
        .await
        .map_err(|_| QueryFailure::new("connect", "query_peer_or_protocol_invalid", started))?;
    let connect_ms = millis(phase.elapsed());
    current_phase.store(2, Ordering::Relaxed);
    let phase = Instant::now();
    query_client::write_query(&mut reader, &context.runtime, request, max_bytes)
        .await
        .map_err(|_| QueryFailure::new("write", "request_write_failed", started))?;
    let write_ms = millis(phase.elapsed());
    current_phase.store(3, Ordering::Relaxed);
    let phase = Instant::now();
    let line = framing::line(&mut reader, max_bytes)
        .await
        .map_err(|error| {
            QueryFailure::new(
                "read",
                match error {
                    framing::FrameError::Oversize => "response_limit_exceeded",
                    framing::FrameError::Eof | framing::FrameError::Incomplete => {
                        "response_frame_incomplete"
                    }
                    _ => "response_read_failed",
                },
                started,
            )
        })?;
    let wait_read_ms = millis(phase.elapsed());
    if line.len() > max_bytes {
        return Err(QueryFailure::new(
            "read",
            "response_limit_exceeded",
            started,
        ));
    }
    if !line.ends_with(b"\n") {
        return Err(QueryFailure::new(
            "read",
            "response_frame_incomplete",
            started,
        ));
    }
    current_phase.store(4, Ordering::Relaxed);
    let phase = Instant::now();
    let response =
        query_client::decode_query_response(&line, &context.runtime).map_err(|error| {
            QueryFailure::new(
                "parse",
                if matches!(
                    error.downcast_ref::<framing::FrameError>(),
                    Some(framing::FrameError::Oversize)
                ) {
                    "response_limit_exceeded"
                } else {
                    "response_json_invalid"
                },
                started,
            )
        })?;
    // 不输出后台原始错误串，避免把内部配置/敏感调试数据带到终端。
    if !response.ok {
        return Err(QueryFailure::new(
            "daemon",
            "daemon_request_failed",
            started,
        ));
    }
    if !response.data.is_object() {
        return Err(QueryFailure::new(
            "parse",
            "response_object_required",
            started,
        ));
    }
    let parse_ms = millis(phase.elapsed());
    Ok(Reply {
        data: response.data,
        timing: RequestTiming {
            serialize_ms: Some(serialize_ms),
            connect_ms: Some(connect_ms),
            write_ms: Some(write_ms),
            wait_read_ms: Some(wait_read_ms),
            parse_ms: Some(parse_ms),
            authenticated_roundtrip_ms: None,
            total_ms: millis(started.elapsed()),
            response_bytes: line.len(),
        },
    })
}

#[cfg(not(windows))]
async fn exchange(
    _: &FixedRuntimeContext,
    _: Request,
    _: usize,
    _: &AtomicU8,
    _: &std::sync::Mutex<Option<String>>,
) -> std::result::Result<Reply, QueryFailure> {
    Err(QueryFailure::new(
        "platform",
        "windows_required",
        Instant::now(),
    ))
}

fn phase_name(phase: &AtomicU8) -> &'static str {
    match phase.load(Ordering::Relaxed) {
        1 => "connect",
        2 => "write",
        3 => "read",
        4 => "parse",
        5 => "authenticated_service",
        _ => "serialize",
    }
}
