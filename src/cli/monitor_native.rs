//! 将监控和延迟采样参数封装为类型化操作，交由统一操作客户端执行。
pub use crate::daemon::operations::monitor_native::{Args, LatencyArgs};
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_monitor(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::Monitor { args })
}

pub fn cmd_latency(args: LatencyArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::Latency { args })
}
