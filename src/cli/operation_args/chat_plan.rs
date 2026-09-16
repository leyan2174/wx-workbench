use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Estimate,
    Scan,
}

#[derive(Debug, clap::Args, Clone)]
pub struct Args {
    /// 明确指定已解密缓存根目录；不读取配置或发现账号
    #[arg(long)]
    pub decrypted_dir: PathBuf,
    /// 相对解密根目录的消息数据库路径，可重复；不指定时保留缺库状态
    #[arg(long = "message-db")]
    pub message_dbs: Vec<PathBuf>,
    /// 相对解密根目录的 message_resource 数据库路径
    #[arg(long)]
    pub resource_db: Option<PathBuf>,
    /// 相对解密根目录的语音数据库路径，可重复
    #[arg(long = "media-db")]
    pub media_dbs: Vec<PathBuf>,
    /// 明确的 username，可重复；提供元数据清单时作为精确过滤
    #[arg(long = "user", required_unless_present = "chats_json")]
    pub users: Vec<String>,
    /// 聊天元数据 JSON 数组：username、index、chat_name/display_name、chat_type/kind
    #[arg(long)]
    pub chats_json: Option<PathBuf>,
    /// 精确排除 username，可重复
    #[arg(long = "exclude-user")]
    pub exclude_users: Vec<String>,
    /// 统计模式；scan 另需显式源目录或媒体目录
    #[arg(long, value_enum, default_value = "estimate")]
    pub size_mode: Mode,
    /// 账号源目录，扫描其 msg 子目录；与 media-dir 二选一
    #[arg(long, conflicts_with = "media_dir")]
    pub source_dir: Option<PathBuf>,
    /// 直接指定包含 attach/file/video 的媒体目录
    #[arg(long, conflicts_with = "source_dir")]
    pub media_dir: Option<PathBuf>,
    /// 扫描线程数，结果仍按清单顺序输出
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=6))]
    pub threads: u8,
    /// 含端点的起始本地时间：日期、日期时间或 Unix 秒
    #[arg(long, allow_hyphen_values = true)]
    pub start: Option<String>,
    /// 含端点的结束时间；仅日期表示当天零点
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<String>,
    /// 新 CSV 文件；父目录须存在，禁止覆盖或写入源目录
    #[arg(short, long)]
    pub output: PathBuf,
}

impl From<Mode> for crate::service::operation_requests::chat_plan::Mode {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Estimate => Self::Estimate,
            Mode::Scan => Self::Scan,
        }
    }
}

impl From<crate::service::operation_requests::chat_plan::Mode> for Mode {
    fn from(value: crate::service::operation_requests::chat_plan::Mode) -> Self {
        match value {
            crate::service::operation_requests::chat_plan::Mode::Estimate => Self::Estimate,
            crate::service::operation_requests::chat_plan::Mode::Scan => Self::Scan,
        }
    }
}

impl From<Args> for crate::service::operation_requests::chat_plan::Args {
    fn from(value: Args) -> Self {
        Self {
            decrypted_dir: value.decrypted_dir,
            message_dbs: value.message_dbs,
            resource_db: value.resource_db,
            media_dbs: value.media_dbs,
            users: value.users,
            chats_json: value.chats_json,
            exclude_users: value.exclude_users,
            size_mode: value.size_mode.into(),
            source_dir: value.source_dir,
            media_dir: value.media_dir,
            threads: value.threads,
            start: value.start,
            end: value.end,
            output: value.output,
        }
    }
}

impl From<crate::service::operation_requests::chat_plan::Args> for Args {
    fn from(value: crate::service::operation_requests::chat_plan::Args) -> Self {
        Self {
            decrypted_dir: value.decrypted_dir,
            message_dbs: value.message_dbs,
            resource_db: value.resource_db,
            media_dbs: value.media_dbs,
            users: value.users,
            chats_json: value.chats_json,
            exclude_users: value.exclude_users,
            size_mode: value.size_mode.into(),
            source_dir: value.source_dir,
            media_dir: value.media_dir,
            threads: value.threads,
            start: value.start,
            end: value.end,
            output: value.output,
        }
    }
}
