#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub sample: super::image_key_sample::SampleArgs,
    /// 明确授权读取当前微信进程内存
    #[arg(long, required_unless_present = "offline", conflicts_with = "offline")]
    pub authorize_memory_scan: bool,
    /// 仅从当前账号目录及图片缓存推导密钥，不读取进程内存
    #[arg(long)]
    pub offline: bool,
    /// 仅提取和验证，不保存密钥
    #[arg(long)]
    pub no_save: bool,
    /// 离线推导或全部候选进程共用的时间预算（秒）
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub timeout: u64,
    /// 图片缓存与候选进程共用的读取预算（MiB）
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u64).range(1..=32768))]
    pub max_mib: u64,
}

#[derive(Debug, clap::Args, Clone)]
pub struct MonitorArgs {
    #[command(flatten)]
    sample: super::image_key_sample::SampleArgs,
    /// 明确授权在监控期间重复读取当前微信进程内存
    #[arg(long, required = true)]
    authorize_memory_scan: bool,
    /// 找到后仅验证，不保存密钥
    #[arg(long)]
    no_save: bool,
    /// 单轮扫描上限（秒）；取消时等待当前有界扫描回收资源
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=30))]
    scan_seconds: u64,
    /// 两轮扫描之间的等待时间（毫秒）
    #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(100..=60000))]
    interval_ms: u64,
    /// 整个监控的时间上限（秒）
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=86400))]
    timeout: u64,
    /// 每轮最多读取的内存（MiB）
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u64).range(1..=32768))]
    max_mib: u64,
}

impl From<Args> for crate::service::operation_requests::image_keys::Args {
    fn from(value: Args) -> Self {
        Self {
            sample: value.sample.into(),
            authorize_memory_scan: value.authorize_memory_scan,
            offline: value.offline,
            no_save: value.no_save,
            timeout: value.timeout,
            max_mib: value.max_mib,
        }
    }
}

impl From<crate::service::operation_requests::image_keys::Args> for Args {
    fn from(value: crate::service::operation_requests::image_keys::Args) -> Self {
        Self {
            sample: value.sample.into(),
            authorize_memory_scan: value.authorize_memory_scan,
            offline: value.offline,
            no_save: value.no_save,
            timeout: value.timeout,
            max_mib: value.max_mib,
        }
    }
}

impl From<MonitorArgs> for crate::service::operation_requests::image_keys::MonitorArgs {
    fn from(value: MonitorArgs) -> Self {
        Self {
            sample: value.sample.into(),
            authorize_memory_scan: value.authorize_memory_scan,
            no_save: value.no_save,
            scan_seconds: value.scan_seconds,
            interval_ms: value.interval_ms,
            timeout: value.timeout,
            max_mib: value.max_mib,
        }
    }
}

impl From<crate::service::operation_requests::image_keys::MonitorArgs> for MonitorArgs {
    fn from(value: crate::service::operation_requests::image_keys::MonitorArgs) -> Self {
        Self {
            sample: value.sample.into(),
            authorize_memory_scan: value.authorize_memory_scan,
            no_save: value.no_save,
            scan_seconds: value.scan_seconds,
            interval_ms: value.interval_ms,
            timeout: value.timeout,
            max_mib: value.max_mib,
        }
    }
}
