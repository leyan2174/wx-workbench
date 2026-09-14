/// CLI 解析和后台执行共用字段定义；IPC 仍保留原来的扁平字段。
#[derive(clap::Args)]
pub struct Args {
    /// 会话名称（可选；省略则导出全部语音）
    pub chat: Option<String>,
    /// 输出目录
    #[arg(short = 'o', long)]
    pub output: String,
    /// 最多导出条数
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,
    /// 分页偏移
    #[arg(long, default_value = "0")]
    pub offset: usize,
    /// 起始时间 YYYY-MM-DD
    #[arg(long)]
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    #[arg(long)]
    pub until: Option<String>,
    /// 目标已存在时覆盖
    #[arg(long)]
    pub overwrite: bool,
    /// 输出 JSON（默认 YAML）
    #[arg(long)]
    pub json: bool,
}

impl From<Args> for crate::service::operation_requests::voices::Args {
    fn from(value: Args) -> Self {
        Self {
            chat: value.chat,
            output: value.output,
            limit: value.limit,
            offset: value.offset,
            since: value.since,
            until: value.until,
            overwrite: value.overwrite,
            json: value.json,
        }
    }
}

impl From<crate::service::operation_requests::voices::Args> for Args {
    fn from(value: crate::service::operation_requests::voices::Args) -> Self {
        Self {
            chat: value.chat,
            output: value.output,
            limit: value.limit,
            offset: value.offset,
            since: value.since,
            until: value.until,
            overwrite: value.overwrite,
            json: value.json,
        }
    }
}
