//! 校验监控和延迟采样参数，在固定账号上下文中运行采样循环并输出结果。
//! 增量查询由监控工具层通过认证通道发送，大状态使用分块传输。
use crate::application::monitor::{
    self, latency::LatencyOptions, FixedRuntimeContext, MonitorOptions,
};
use crate::infrastructure::cancellation::ConsoleCancellation;
use anyhow::{ensure, Result};

pub use crate::service::operation_requests::monitor_native::Args;

pub use crate::service::operation_requests::monitor_native::LatencyArgs;

pub fn cmd_monitor(args: Args) -> Result<()> {
    let options = args.options()?;
    let context = FixedRuntimeContext::load()?;
    cmd_monitor_for(&context, &options, args.json, args.max_content_chars)
}

/// main 已固定账号时直接传入上下文，整个连续操作不重新读取配置。
pub fn cmd_monitor_for(
    context: &FixedRuntimeContext,
    options: &MonitorOptions,
    json: bool,
    max_content_chars: usize,
) -> Result<()> {
    options.validate()?;
    ensure!(
        !options.emit_state || json,
        "--emit-state 仅用于 JSONL 输出"
    );
    ensure!(
        (32..=1_000_000).contains(&max_content_chars),
        "文本显示字符上限无效"
    );
    let console = ConsoleCancellation::install()?;
    let token = console.token();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut sink = |event| {
        monitor::render(
            &event,
            json,
            max_content_chars,
            &mut std::io::stdout().lock(),
        )
    };
    let summary = runtime.block_on(monitor::run_monitor(context, options, &token, &mut sink))?;
    ensure!(
        summary.stop_reason != "error_limit",
        "达到连续错误上限；原游标未重置，详见事件流"
    );
    Ok(())
}

pub fn cmd_latency(args: LatencyArgs) -> Result<()> {
    let options = args.options()?;
    let context = FixedRuntimeContext::load()?;
    cmd_latency_for(&context, &options, args.json)
}
pub fn cmd_latency_for(
    context: &FixedRuntimeContext,
    options: &LatencyOptions,
    json: bool,
) -> Result<()> {
    options.validate()?;
    let console = ConsoleCancellation::install()?;
    let token = console.token();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut sink = |event| monitor::render(&event, json, 4000, &mut std::io::stdout().lock());
    let summary = runtime.block_on(monitor::latency::run_latency(
        context, options, &token, &mut sink,
    ))?;
    ensure!(
        summary.successful_samples > 0 || summary.stop_reason == "cancelled",
        "观测窗口内没有成功 IPC 样本，不能给出延迟结论"
    );
    Ok(())
}
