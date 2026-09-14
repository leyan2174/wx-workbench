#[derive(Debug, clap::Args, Clone)]
pub struct Args {
    /// 明确授权只读扫描配置中微信进程的内存
    #[arg(long, required = true)]
    pub authorize_memory_scan: bool,
}

impl From<Args> for crate::service::operation_requests::database_keys::Args {
    fn from(value: Args) -> Self {
        Self {
            authorize_memory_scan: value.authorize_memory_scan,
        }
    }
}

impl From<crate::service::operation_requests::database_keys::Args> for Args {
    fn from(value: crate::service::operation_requests::database_keys::Args) -> Self {
        Self {
            authorize_memory_scan: value.authorize_memory_scan,
        }
    }
}
