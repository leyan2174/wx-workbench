use std::path::PathBuf;

#[derive(Clone, Copy, clap::ValueEnum, Debug)]
pub enum Initial {
    Now,
    Recent,
}

#[derive(clap::Args, Clone, Debug)]
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

#[derive(clap::Args, Clone, Debug)]
pub struct LatencyArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long = "duration", default_value_t = 60)]
    pub duration_secs: u64,
    /// 默认每 30 ms 检查 mtime；不读取 DB/WAL 内容
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

impl From<Initial> for crate::service::operation_requests::monitor_native::Initial {
    fn from(value: Initial) -> Self {
        match value {
            Initial::Now => Self::Now,
            Initial::Recent => Self::Recent,
        }
    }
}

impl From<crate::service::operation_requests::monitor_native::Initial> for Initial {
    fn from(value: crate::service::operation_requests::monitor_native::Initial) -> Self {
        match value {
            crate::service::operation_requests::monitor_native::Initial::Now => Self::Now,
            crate::service::operation_requests::monitor_native::Initial::Recent => Self::Recent,
        }
    }
}

impl From<Args> for crate::service::operation_requests::monitor_native::Args {
    fn from(value: Args) -> Self {
        Self {
            json: value.json,
            initial: value.initial.into(),
            state_file: value.state_file,
            emit_state: value.emit_state,
            interval_ms: value.interval_ms,
            limit: value.limit,
            max_limit: value.max_limit,
            timeout_ms: value.timeout_ms,
            max_response_mib: value.max_response_mib,
            with_meta: value.with_meta,
            debug_source: value.debug_source,
            max_cycles: value.max_cycles,
            duration_secs: value.duration_secs,
            max_consecutive_errors: value.max_consecutive_errors,
            max_content_chars: value.max_content_chars,
        }
    }
}

impl From<crate::service::operation_requests::monitor_native::Args> for Args {
    fn from(value: crate::service::operation_requests::monitor_native::Args) -> Self {
        Self {
            json: value.json,
            initial: value.initial.into(),
            state_file: value.state_file,
            emit_state: value.emit_state,
            interval_ms: value.interval_ms,
            limit: value.limit,
            max_limit: value.max_limit,
            timeout_ms: value.timeout_ms,
            max_response_mib: value.max_response_mib,
            with_meta: value.with_meta,
            debug_source: value.debug_source,
            max_cycles: value.max_cycles,
            duration_secs: value.duration_secs,
            max_consecutive_errors: value.max_consecutive_errors,
            max_content_chars: value.max_content_chars,
        }
    }
}

impl From<LatencyArgs> for crate::service::operation_requests::monitor_native::LatencyArgs {
    fn from(value: LatencyArgs) -> Self {
        Self {
            json: value.json,
            duration_secs: value.duration_secs,
            poll_interval_ms: value.poll_interval_ms,
            probe_interval_ms: value.probe_interval_ms,
            timeout_ms: value.timeout_ms,
            max_response_mib: value.max_response_mib,
            sessions_limit: value.sessions_limit,
            history_limit: value.history_limit,
            chat: value.chat,
            ipc_only: value.ipc_only,
            debug_source: value.debug_source,
        }
    }
}

impl From<crate::service::operation_requests::monitor_native::LatencyArgs> for LatencyArgs {
    fn from(value: crate::service::operation_requests::monitor_native::LatencyArgs) -> Self {
        Self {
            json: value.json,
            duration_secs: value.duration_secs,
            poll_interval_ms: value.poll_interval_ms,
            probe_interval_ms: value.probe_interval_ms,
            timeout_ms: value.timeout_ms,
            max_response_mib: value.max_response_mib,
            sessions_limit: value.sessions_limit,
            history_limit: value.history_limit,
            chat: value.chat,
            ipc_only: value.ipc_only,
            debug_source: value.debug_source,
        }
    }
}
