use super::plan::Mode;
use std::path::PathBuf;

#[derive(clap::Parser, Clone, Debug)]
pub struct Args {
    /// 输出目录；默认选中配置旁 exported_chats
    pub output_dir: Option<PathBuf>,
    /// 导出时按数据库身份关联并转录语音
    #[arg(short = 't', long)]
    pub with_transcriptions: bool,
    /// 生成计划 CSV，不导出聊天
    #[arg(long, conflicts_with = "from_plan_csv")]
    pub write_plan_csv: Option<PathBuf>,
    /// 读取计划 CSV，按 username 选择聊天
    #[arg(long)]
    pub from_plan_csv: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "blacklist")]
    pub plan_mode: Mode,
    #[arg(long, value_enum, default_value = "estimate")]
    pub size_mode: super::chat_plan::Mode,
    /// 保留旧消息并追加本轮新消息
    #[arg(short = 'i', long)]
    pub incremental: bool,
    /// 只写新的 delta 批次，不读取或改写完整聊天文件
    #[arg(long, requires = "start")]
    pub delta_only: bool,
    #[arg(long, allow_hyphen_values = true)]
    pub start: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub users: Option<String>,
    #[command(flatten)]
    pub asr: super::asr_batch::BatchArgs,
}

impl From<Args> for crate::service::operation_requests::export_all::Args {
    fn from(value: Args) -> Self {
        Self {
            output_dir: value.output_dir,
            with_transcriptions: value.with_transcriptions,
            write_plan_csv: value.write_plan_csv,
            from_plan_csv: value.from_plan_csv,
            plan_mode: value.plan_mode.into(),
            size_mode: value.size_mode.into(),
            incremental: value.incremental,
            delta_only: value.delta_only,
            start: value.start,
            end: value.end,
            dry_run: value.dry_run,
            users: value.users,
            asr: value.asr.into(),
        }
    }
}

impl From<crate::service::operation_requests::export_all::Args> for Args {
    fn from(value: crate::service::operation_requests::export_all::Args) -> Self {
        Self {
            output_dir: value.output_dir,
            with_transcriptions: value.with_transcriptions,
            write_plan_csv: value.write_plan_csv,
            from_plan_csv: value.from_plan_csv,
            plan_mode: value.plan_mode.into(),
            size_mode: value.size_mode.into(),
            incremental: value.incremental,
            delta_only: value.delta_only,
            start: value.start,
            end: value.end,
            dry_run: value.dry_run,
            users: value.users,
            asr: value.asr.into(),
        }
    }
}
