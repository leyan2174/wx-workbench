//! 校验监控和延迟采样参数，在固定账号上下文中运行采样循环并输出结果。
//! 增量查询由监控工具层通过认证通道发送，大状态使用分块传输。
use crate::toolkit::monitor::{
    self, latency::LatencyOptions, ConsoleCancellation, FixedRuntimeContext, InitialPolicy,
    MonitorOptions,
};
use anyhow::{ensure, Result};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy, clap::ValueEnum, serde::Serialize, serde::Deserialize, Debug)]
pub enum Initial {
    Now,
    Recent,
}

#[derive(clap::Args, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Args {
    /// JSON Lines 事件流，每行一个完整 JSON 对象
    #[arg(long)]
    pub json: bool,
    /// 当前基线或回看最近 24 小时；不等于完整历史回放
    #[arg(long, value_enum, default_value = "now")]
    pub initial: Initial,
    /// 只读导入 version/runtime_id/sessions；不写回、不兼容未绑定账号的旧状态
    #[arg(long)]
    pub state_file: Option<PathBuf>,
    /// 将已提交的账号绑定状态附在 JSONL 事件中，不写文件
    #[arg(long, requires = "json")]
    pub emit_state: bool,
    #[arg(long, default_value_t = 3000)]
    pub interval_ms: u64,
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// 满额时保持游标并逐轮增大 limit，最多到此值
    #[arg(long, default_value_t = 10000)]
    pub max_limit: usize,
    #[arg(long, default_value_t = 20000)]
    pub timeout_ms: u64,
    #[arg(long, default_value_t = 8)]
    pub max_response_mib: usize,
    #[arg(long)]
    pub with_meta: bool,
    #[arg(long)]
    pub debug_source: bool,
    /// 基线和失败轮询也计入次数；不指定则持续至 Ctrl+C
    #[arg(long)]
    pub max_cycles: Option<u64>,
    #[arg(long = "duration")]
    pub duration_secs: Option<u64>,
    #[arg(long)]
    pub max_consecutive_errors: Option<u64>,
    /// 仅限制文本模式单字段显示；截断会明确标注
    #[arg(long, default_value_t = 4000)]
    pub max_content_chars: usize,
}
impl Args {
    pub fn options(&self) -> Result<MonitorOptions> {
        ensure!(
            !self.emit_state || self.json,
            "--emit-state 仅用于 JSONL 输出"
        );
        ensure!(
            (32..=1_000_000).contains(&self.max_content_chars),
            "文本显示字符上限须在 32..1000000 内"
        );
        let options = MonitorOptions {
            interval: Duration::from_millis(self.interval_ms),
            request_timeout: Duration::from_millis(self.timeout_ms),
            limit: self.limit,
            max_limit: self.max_limit,
            max_response_bytes: response_bytes(self.max_response_mib)?,
            initial: match self.initial {
                Initial::Now => InitialPolicy::Now,
                Initial::Recent => InitialPolicy::Recent,
            },
            state_file: self.state_file.clone(),
            with_meta: self.with_meta,
            debug_source: self.debug_source,
            emit_state: self.emit_state,
            max_cycles: self.max_cycles,
            max_duration: self.duration_secs.map(Duration::from_secs),
            max_consecutive_errors: self.max_consecutive_errors,
        };
        options.validate()?;
        Ok(options)
    }
}

#[derive(clap::Args, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LatencyArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long = "duration", default_value_t = 60)]
    pub duration_secs: u64,
    /// 与 legacy 相同，默认 30 ms 检查 mtime；不读取 DB/WAL 内容
    #[arg(long, default_value_t = 30)]
    pub poll_interval_ms: u64,
    /// 即使无文件变化，也按此间隔采样 IPC
    #[arg(long, default_value_t = 1000)]
    pub probe_interval_ms: u64,
    #[arg(long, default_value_t = 20000)]
    pub timeout_ms: u64,
    #[arg(long, default_value_t = 8)]
    pub max_response_mib: usize,
    #[arg(long, default_value_t = 200)]
    pub sessions_limit: usize,
    #[arg(long, default_value_t = 20)]
    pub history_limit: usize,
    /// 指定后额外采样 History，并比较 session/history 元数据时间戳
    #[arg(long)]
    pub chat: Option<String>,
    /// 只测 IPC，不观察账号 DB/WAL 文件元数据
    #[arg(long)]
    pub ipc_only: bool,
    #[arg(long)]
    pub debug_source: bool,
}
impl LatencyArgs {
    pub fn options(&self) -> Result<LatencyOptions> {
        let options = LatencyOptions {
            duration: Duration::from_secs(self.duration_secs),
            poll_interval: Duration::from_millis(self.poll_interval_ms),
            probe_interval: Duration::from_millis(self.probe_interval_ms),
            request_timeout: Duration::from_millis(self.timeout_ms),
            max_response_bytes: response_bytes(self.max_response_mib)?,
            sessions_limit: self.sessions_limit,
            history_limit: self.history_limit,
            chat: self.chat.clone(),
            ipc_only: self.ipc_only,
            debug_source: self.debug_source,
        };
        options.validate()?;
        Ok(options)
    }
}

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

fn response_bytes(mib: usize) -> Result<usize> {
    ensure!((1..=64).contains(&mib), "响应大小上限须在 1..64 MiB 内");
    Ok(mib * 1024 * 1024)
}
