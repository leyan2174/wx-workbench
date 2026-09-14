use anyhow::{ensure, Result};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Args {
    pub sample: super::image_key_sample::SampleArgs,
    /// 明确授权读取当前微信进程内存
    pub authorize_memory_scan: bool,
    /// 仅从当前账号目录及图片缓存推导密钥，不读取进程内存
    pub offline: bool,
    /// 仅提取和验证，不保存密钥
    pub no_save: bool,
    /// 离线推导或全部候选进程共用的时间预算（秒）
    pub timeout: u64,
    /// 图片缓存与候选进程共用的读取预算（MiB）
    pub max_mib: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct MonitorArgs {
    pub(crate) sample: super::image_key_sample::SampleArgs,
    /// 明确授权在监控期间重复读取当前微信进程内存
    pub(crate) authorize_memory_scan: bool,
    /// 找到后仅验证，不保存密钥
    pub(crate) no_save: bool,
    /// 单轮扫描上限（秒）；取消时等待当前有界扫描回收资源
    pub(crate) scan_seconds: u64,
    /// 两轮扫描之间的等待时间（毫秒）
    pub(crate) interval_ms: u64,
    /// 整个监控的时间上限（秒）
    pub(crate) timeout: u64,
    /// 每轮最多读取的内存（MiB）
    pub(crate) max_mib: u64,
}

impl MonitorArgs {
    pub(crate) fn saves_keys(&self) -> bool {
        !self.no_save
    }

    pub(crate) fn validate_request(&self) -> Result<()> {
        ensure!(self.authorize_memory_scan, "图片密钥监控需要明确授权");
        ensure!(
            (1..=30).contains(&self.scan_seconds)
                && (100..=60000).contains(&self.interval_ms)
                && (1..=86400).contains(&self.timeout)
                && (1..=32768).contains(&self.max_mib),
            "扫描预算超出允许范围"
        );
        self.sample.validate_request()
    }
}
