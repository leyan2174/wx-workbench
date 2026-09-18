use std::path::PathBuf;

#[derive(Default, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LocalCacheArgs {
    /// 本账号 xwechat 缓存根目录；显式提供时才扫描
    pub(crate) xwechat_cache: Option<PathBuf>,
    /// 本账号 FileStorage/Sns 缓存根目录
    pub(crate) sns_cache: Option<PathBuf>,
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
            crate::application::image_publication::parse_xor(raw.trim())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_material_fields_are_rejected_but_xor_format_is_preserved() {
        for field in ["image_key_file", "aes_key", "allow_material_import"] {
            let mut value = serde_json::json!({});
            value[field] = serde_json::json!("synthetic");
            assert!(serde_json::from_value::<LocalCacheArgs>(value).is_err());
        }
        for raw in ["0", "255", "0x88", "0Xff"] {
            let args: LocalCacheArgs =
                serde_json::from_value(serde_json::json!({"image_xor_key":raw})).unwrap();
            args.validate_request().unwrap();
            assert!(serde_json::to_value(args)
                .unwrap()
                .get("image_key_file")
                .is_none());
        }
        for raw in ["-1", "256", "0x100", "not-a-byte"] {
            let args: LocalCacheArgs =
                serde_json::from_value(serde_json::json!({"image_xor_key":raw})).unwrap();
            assert!(args.validate_request().is_err());
        }
    }
}
