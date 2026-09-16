use crate::application::monitor::{latency::LatencyOptions, InitialPolicy, MonitorOptions};
use anyhow::{ensure, Result};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Debug)]
pub enum Initial {
    Now,
    Recent,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Args {
    /// JSON Lines 事件流，每行一个完整 JSON 对象
    pub json: bool,
    /// 当前基线或回看最近 24 小时；不等于完整历史回放
    pub initial: Initial,
    /// 只读导入 version/runtime_id/sessions；不写回、不兼容未绑定账号的旧状态
    pub state_file: Option<PathBuf>,
    /// 将已提交的账号绑定状态附在 JSONL 事件中，不写文件
    pub emit_state: bool,
    pub interval_ms: u64,
    pub limit: usize,
    /// 满额时保持游标并逐轮增大 limit，最多到此值
    pub max_limit: usize,
    pub timeout_ms: u64,
    pub max_response_mib: usize,
    pub with_meta: bool,
    pub debug_source: bool,
    /// 基线和失败轮询也计入次数；不指定则持续至 Ctrl+C
    pub max_cycles: Option<u64>,
    pub duration_secs: Option<u64>,
    pub max_consecutive_errors: Option<u64>,
    /// 仅限制文本模式单字段显示；截断会明确标注
    pub max_content_chars: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LatencyArgs {
    pub json: bool,
    pub duration_secs: u64,
    /// 与 legacy 相同，默认 30 ms 检查 mtime；不读取 DB/WAL 内容
    pub poll_interval_ms: u64,
    /// 即使无文件变化，也按此间隔采样 IPC
    pub probe_interval_ms: u64,
    pub timeout_ms: u64,
    pub max_response_mib: usize,
    pub sessions_limit: usize,
    pub history_limit: usize,
    /// 指定后额外采样 History，并比较 session/history 元数据时间戳
    pub chat: Option<String>,
    /// 只测 IPC，不观察账号 DB/WAL 文件元数据
    pub ipc_only: bool,
    pub debug_source: bool,
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

fn response_bytes(mib: usize) -> Result<usize> {
    ensure!((1..=64).contains(&mib), "响应大小上限须在 1..64 MiB 内");
    Ok(mib * 1024 * 1024)
}
