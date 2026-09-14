use std::path::PathBuf;

#[derive(Debug, Clone, Default, clap::Args)]
pub struct Args {
    /// 输出根目录；省略时读取当前账号配置的 output_base_dir
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
    /// 精确 username 或目录中的 unknown_<完整表哈希>，逗号分隔；支持 WECHAT_EXPORT_CONTACTS
    #[arg(long)]
    pub contacts: Option<String>,
    /// csv,html,json，默认全部；也支持 WECHAT_EXPORT_FORMATS
    #[arg(long)]
    pub formats: Option<String>,
    /// 禁用媒体复制与解码，仍保留显式 disabled 标记
    #[arg(long)]
    pub no_media: bool,
    /// 更新本工具已绑定的目录，不认领未知 legacy 目录
    #[arg(long)]
    pub update: bool,
    /// 只列出精确联系人与目标路径
    #[arg(long)]
    pub dry_run: bool,
    /// 有媒体缺失或失败时仍返回成功；诊断始终保留
    #[arg(long)]
    pub allow_missing_media: bool,
    /// 单个附件最大字节数，默认 64 MiB
    #[arg(long)]
    pub max_media_bytes: Option<u64>,
    /// 每个聊天累计媒体最大字节数，默认 2 GiB
    #[arg(long)]
    pub max_total_media_bytes: Option<u64>,
}

impl From<Args> for crate::service::operation_requests::export_messages::Args {
    fn from(value: Args) -> Self {
        Self {
            output_dir: value.output_dir,
            contacts: value.contacts,
            formats: value.formats,
            no_media: value.no_media,
            update: value.update,
            dry_run: value.dry_run,
            allow_missing_media: value.allow_missing_media,
            max_media_bytes: value.max_media_bytes,
            max_total_media_bytes: value.max_total_media_bytes,
        }
    }
}

impl From<crate::service::operation_requests::export_messages::Args> for Args {
    fn from(value: crate::service::operation_requests::export_messages::Args) -> Self {
        Self {
            output_dir: value.output_dir,
            contacts: value.contacts,
            formats: value.formats,
            no_media: value.no_media,
            update: value.update,
            dry_run: value.dry_run,
            allow_missing_media: value.allow_missing_media,
            max_media_bytes: value.max_media_bytes,
            max_total_media_bytes: value.max_total_media_bytes,
        }
    }
}
