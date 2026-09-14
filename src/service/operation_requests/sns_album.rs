use std::path::PathBuf;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 作者昵称、备注名或微信 ID
    pub user: String,
    /// 输出根目录；在其下创建带时间戳的相册目录
    pub output: PathBuf,
    /// 指定相册目录；已绑定同一账号和联系人的目录可更新
    pub output_dir: Option<PathBuf>,
    /// 明确认领尚无来源绑定的旧相册；不能覆盖已知账号或联系人冲突
    pub adopt_existing: bool,
    /// 最多读取的朋友圈条数
    pub limit: usize,
    /// 图片工作线程数，按旧规则限制到 1 至 32
    pub image_workers: i64,
    /// 视频工作线程数，按旧规则限制到 1 至 16
    pub video_workers: i64,
    /// 起始时间 YYYY-MM-DD
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    pub until: Option<String>,
    /// 不进行图片或视频网络下载；仍可复用已有媒体及本账号视频缓存
    pub no_remote: bool,
    /// 不处理视频
    pub no_videos: bool,
}
