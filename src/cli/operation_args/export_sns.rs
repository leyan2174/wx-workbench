use std::path::PathBuf;

#[derive(clap::Args, Default, Clone, Debug)]
pub struct LocalCacheArgs {
    /// 本账号 xwechat 缓存根目录；显式提供时才扫描
    #[arg(long)]
    xwechat_cache: Option<PathBuf>,
    /// 本账号 FileStorage/Sns 缓存根目录
    #[arg(long)]
    sns_cache: Option<PathBuf>,
    /// V2 图片 AES 密钥文件，UTF-8 文本含 32 位十六进制
    #[arg(long)]
    image_key_file: Option<PathBuf>,
    /// V2 图片尾部 XOR 字节，十进制或 0x 前缀；默认 0x88
    #[arg(long)]
    image_xor_key: Option<String>,
}

#[derive(clap::Args)]
pub struct Args {
    /// 已解密 SNS SQLite 数据库
    pub sns_db: PathBuf,
    /// 新输出目录，须位于源数据库目录之外
    pub output_dir: PathBuf,
    /// 已解密联系人数据库
    #[arg(long)]
    pub contact_db: Option<PathBuf>,
    /// 按 user_name 筛选，逗号分隔；默认读取 WECHAT_EXPORT_CONTACTS
    #[arg(long)]
    pub contacts: Option<String>,
    /// 可选固定时区偏移，例如 +08:00；默认本机时区
    #[arg(long, allow_hyphen_values = true)]
    pub utc_offset: Option<String>,
    /// 显式授权下载未从本地缓存恢复的媒体；默认不联网
    #[arg(long)]
    pub download_media: bool,
    /// 创建或更新绑定同一数据库来源的时间线；默认仍为 fresh 整目录发布
    #[arg(long)]
    pub update: bool,
    /// 显式认领无来源绑定的旧时间线；已知来源冲突仍拒绝
    #[arg(long, requires = "update")]
    pub adopt_existing: bool,
    #[command(flatten)]
    pub local_cache: LocalCacheArgs,
}

impl From<LocalCacheArgs> for crate::service::operation_requests::export_sns::LocalCacheArgs {
    fn from(value: LocalCacheArgs) -> Self {
        Self {
            xwechat_cache: value.xwechat_cache,
            sns_cache: value.sns_cache,
            image_key_file: value.image_key_file,
            image_xor_key: value.image_xor_key,
        }
    }
}

impl From<crate::service::operation_requests::export_sns::LocalCacheArgs> for LocalCacheArgs {
    fn from(value: crate::service::operation_requests::export_sns::LocalCacheArgs) -> Self {
        Self {
            xwechat_cache: value.xwechat_cache,
            sns_cache: value.sns_cache,
            image_key_file: value.image_key_file,
            image_xor_key: value.image_xor_key,
        }
    }
}

impl From<Args> for crate::service::operation_requests::export_sns::Args {
    fn from(value: Args) -> Self {
        Self {
            sns_db: value.sns_db,
            output_dir: value.output_dir,
            contact_db: value.contact_db,
            contacts: value.contacts,
            utc_offset: value.utc_offset,
            download_media: value.download_media,
            update: value.update,
            adopt_existing: value.adopt_existing,
            local_cache: value.local_cache.into(),
        }
    }
}

impl From<crate::service::operation_requests::export_sns::Args> for Args {
    fn from(value: crate::service::operation_requests::export_sns::Args) -> Self {
        Self {
            sns_db: value.sns_db,
            output_dir: value.output_dir,
            contact_db: value.contact_db,
            contacts: value.contacts,
            utc_offset: value.utc_offset,
            download_media: value.download_media,
            update: value.update,
            adopt_existing: value.adopt_existing,
            local_cache: value.local_cache.into(),
        }
    }
}
