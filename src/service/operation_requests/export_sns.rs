use std::path::PathBuf;

#[derive(Default, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct LocalCacheArgs {
    /// 本账号 xwechat 缓存根目录；显式提供时才扫描
    pub(crate) xwechat_cache: Option<PathBuf>,
    /// 本账号 FileStorage/Sns 缓存根目录
    pub(crate) sns_cache: Option<PathBuf>,
    /// V2 图片 AES 密钥文件，UTF-8 文本含 32 位十六进制
    pub(crate) image_key_file: Option<PathBuf>,
    /// V2 图片尾部 XOR 字节，十进制或 0x 前缀；默认 0x88
    pub(crate) image_xor_key: Option<String>,
}

pub struct Args {
    /// 已解密 SNS SQLite 数据库
    pub sns_db: PathBuf,
    /// 新输出目录，须位于源数据库目录之外
    pub output_dir: PathBuf,
    /// 已解密联系人数据库
    pub contact_db: Option<PathBuf>,
    /// 按 user_name 筛选，逗号分隔；默认读取 WECHAT_EXPORT_CONTACTS
    pub contacts: Option<String>,
    /// 可选固定时区偏移，例如 +08:00；默认本机时区
    pub utc_offset: Option<String>,
    /// 显式授权下载未从本地缓存恢复的媒体；默认不联网
    pub download_media: bool,
    /// 创建或更新绑定同一数据库来源的时间线；默认仍为 fresh 整目录发布
    pub update: bool,
    /// 显式认领无来源绑定的旧时间线；已知来源冲突仍拒绝
    pub adopt_existing: bool,
    pub local_cache: LocalCacheArgs,
}

impl LocalCacheArgs {
    pub(crate) fn validate_request(&self) -> anyhow::Result<()> {
        if let Some(raw) = &self.image_xor_key {
            crate::toolkit::parse_image_xor(raw.trim())?;
        }
        Ok(())
    }
}
