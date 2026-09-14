use std::path::PathBuf;

#[derive(clap::Args, Clone, Debug)]
pub struct Args {
    /// 输出根目录；默认须全新，--append-run 可复用已有普通目录
    pub output: PathBuf,
    /// 精确 username，逗号分隔；默认 WECHAT_EXPORT_USERS 或全部会话
    #[arg(long)]
    pub users: Option<String>,
    /// 含端点的起始时间：本地日期、日期时间或 Unix 秒
    #[arg(long, allow_hyphen_values = true)]
    pub start: String,
    /// 含端点的结束时间；仅日期表示当天零点
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<String>,
    /// 批次目录名；默认本机时间及纳秒，不接受路径或设备名
    #[arg(long)]
    pub run_id: Option<String>,
    /// 在已有输出根目录中创建全新批次，不覆盖任何已有 run
    #[arg(long)]
    pub append_run: bool,
}

impl From<Args> for crate::service::operation_requests::export_delta::Args {
    fn from(value: Args) -> Self {
        Self {
            output: value.output,
            users: value.users,
            start: value.start,
            end: value.end,
            run_id: value.run_id,
            append_run: value.append_run,
        }
    }
}

impl From<crate::service::operation_requests::export_delta::Args> for Args {
    fn from(value: crate::service::operation_requests::export_delta::Args) -> Self {
        Self {
            output: value.output,
            users: value.users,
            start: value.start,
            end: value.end,
            run_id: value.run_id,
            append_run: value.append_run,
        }
    }
}
