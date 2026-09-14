use super::plan::Mode;
use std::path::PathBuf;

#[derive(Debug, clap::Args, Clone)]
pub struct Args {
    /// 输出目录
    pub output_dir: PathBuf,
    /// 仅导出指定 username，逗号分隔；也可设 WECHAT_EXPORT_USERS
    #[arg(long)]
    pub users: Option<String>,
    /// 保留旧消息并按分片来源和 local_id 追加；身份歧义时拒绝覆盖
    #[arg(short = 'i', long)]
    pub incremental: bool,
    /// 起始本地时间（含端点）：日期、日期时间或 Unix 秒
    #[arg(long)]
    pub start: Option<String>,
    /// 结束本地时间（含端点）；仅日期表示当天零点
    #[arg(long)]
    pub end: Option<String>,
    /// 只列出会话，不创建输出目录或索引
    #[arg(long)]
    pub dry_run: bool,
    /// 读取计划 CSV，以 username 精确选择会话
    #[arg(long)]
    pub from_plan_csv: Option<PathBuf>,
    /// blacklist 仅跳过 export=0（默认）；whitelist 仅选择 export=1
    #[arg(long, value_enum, requires = "from_plan_csv")]
    pub plan_mode: Option<Mode>,
}

impl From<Args> for crate::service::operation_requests::export_chats::Args {
    fn from(value: Args) -> Self {
        Self {
            output_dir: value.output_dir,
            users: value.users,
            incremental: value.incremental,
            start: value.start,
            end: value.end,
            dry_run: value.dry_run,
            from_plan_csv: value.from_plan_csv,
            plan_mode: value.plan_mode.map(Into::into),
        }
    }
}

impl From<crate::service::operation_requests::export_chats::Args> for Args {
    fn from(value: crate::service::operation_requests::export_chats::Args) -> Self {
        Self {
            output_dir: value.output_dir,
            users: value.users,
            incremental: value.incremental,
            start: value.start,
            end: value.end,
            dry_run: value.dry_run,
            from_plan_csv: value.from_plan_csv,
            plan_mode: value.plan_mode.map(Into::into),
        }
    }
}
