//! Authenticated MCP RPC adapter, outside query leases and the async executor.
use crate::{
    mcp::protocol::DispatchError, runtime::RuntimeContext, service::protocol::ServiceError,
};
use serde_json::Value;
use std::sync::{Arc, OnceLock};

pub async fn dispatch(
    call: super::mcp_service::Call,
    runtime: RuntimeContext,
    query: Arc<super::query_state::QueryState>,
) -> Result<Value, ServiceError> {
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(8)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| ServiceError::new("busy", "MCP service is busy"))?;
    let handle = tokio::runtime::Handle::current();
    let response = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        super::mcp_service::dispatch(call, &runtime, |request, context, limit| {
            context.check()?;
            let response = handle
                .block_on(async {
                    tokio::time::timeout(
                        context.remaining(),
                        super::server::dispatch_state(request, &query),
                    )
                    .await
                })
                .map_err(|_| DispatchError::TimedOut)?;
            context.check()?;
            let size = serde_json::to_vec(&response)
                .map_err(|_| DispatchError::InvalidResponse)?
                .len();
            if size > limit {
                return Err(DispatchError::InvalidResponse);
            }
            Ok(response)
        })
    })
    .await
    .map_err(|_| ServiceError::new("unavailable", "MCP worker unavailable"))?;
    serde_json::to_value(response)
        .map_err(|_| ServiceError::new("serialization", "MCP response unavailable"))
}
