use std::path::PathBuf;

#[derive(Debug, Default, clap::Args, Clone)]
pub struct Args {
    /// 按数据库 user_name 精确筛选，多个联系人以逗号分隔
    #[arg(long)]
    pub contacts: Option<String>,
    /// 输出根目录；默认配置旁 wechat_files/<选中账号>
    #[arg(short = 'o', long)]
    pub output_dir: Option<PathBuf>,
    /// 明确认领未绑定来源的旧时间线，不能覆盖已有来源冲突
    #[arg(long)]
    pub adopt_existing: bool,
    /// 授权下载缺失媒体
    #[arg(long, conflicts_with = "no_remote")]
    pub download_media: bool,
    /// 禁止网络下载，优先于 WECHAT_SNS_DOWNLOAD_MEDIA
    #[arg(long, conflicts_with = "download_media")]
    pub no_remote: bool,
}

impl From<Args> for crate::service::operation_requests::sns_timeline::Args {
    fn from(value: Args) -> Self {
        Self {
            contacts: value.contacts,
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
            download_media: value.download_media,
            no_remote: value.no_remote,
        }
    }
}

impl From<crate::service::operation_requests::sns_timeline::Args> for Args {
    fn from(value: crate::service::operation_requests::sns_timeline::Args) -> Self {
        Self {
            contacts: value.contacts,
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
            download_media: value.download_media,
            no_remote: value.no_remote,
        }
    }
}
