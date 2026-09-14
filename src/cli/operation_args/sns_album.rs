use std::path::PathBuf;

#[derive(Debug, clap::Args, Clone)]
pub struct Args {
    /// 作者昵称、备注名或微信 ID
    pub user: String,
    /// 输出根目录；在其下创建带时间戳的相册目录
    #[arg(short = 'o', long, visible_alias = "output-root", default_value = ".")]
    pub output: PathBuf,
    /// 指定相册目录；已绑定同一账号和联系人的目录可更新
    #[arg(long, conflicts_with = "output")]
    pub output_dir: Option<PathBuf>,
    /// 明确认领尚无来源绑定的旧相册；不能覆盖已知账号或联系人冲突
    #[arg(long, requires = "output_dir")]
    pub adopt_existing: bool,
    /// 最多读取的朋友圈条数
    #[arg(short = 'n', long, default_value = "50000")]
    pub limit: usize,
    /// 图片工作线程数，按旧规则限制到 1 至 32
    #[arg(long, default_value = "8", allow_hyphen_values = true)]
    pub image_workers: i64,
    /// 视频工作线程数，按旧规则限制到 1 至 16
    #[arg(long, default_value = "4", allow_hyphen_values = true)]
    pub video_workers: i64,
    /// 起始时间 YYYY-MM-DD
    #[arg(long)]
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    #[arg(long)]
    pub until: Option<String>,
    /// 不进行图片或视频网络下载；仍可复用已有媒体及本账号视频缓存
    #[arg(long)]
    pub no_remote: bool,
    /// 不处理视频
    #[arg(long)]
    pub no_videos: bool,
}

impl From<Args> for crate::service::operation_requests::sns_album::Args {
    fn from(value: Args) -> Self {
        Self {
            user: value.user,
            output: value.output,
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
            limit: value.limit,
            image_workers: value.image_workers,
            video_workers: value.video_workers,
            since: value.since,
            until: value.until,
            no_remote: value.no_remote,
            no_videos: value.no_videos,
        }
    }
}

impl From<crate::service::operation_requests::sns_album::Args> for Args {
    fn from(value: crate::service::operation_requests::sns_album::Args) -> Self {
        Self {
            user: value.user,
            output: value.output,
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
            limit: value.limit,
            image_workers: value.image_workers,
            video_workers: value.video_workers,
            since: value.since,
            until: value.until,
            no_remote: value.no_remote,
            no_videos: value.no_videos,
        }
    }
}
