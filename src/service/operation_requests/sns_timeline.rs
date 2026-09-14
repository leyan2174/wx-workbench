use std::path::PathBuf;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 按数据库 user_name 精确筛选，多个联系人以逗号分隔
    pub contacts: Option<String>,
    /// 输出根目录；默认配置旁 wechat_files/<选中账号>
    pub output_dir: Option<PathBuf>,
    /// 明确认领未绑定来源的旧时间线，不能覆盖已有来源冲突
    pub adopt_existing: bool,
    /// 授权下载缺失媒体
    pub download_media: bool,
    /// 禁止网络下载，优先于 WECHAT_SNS_DOWNLOAD_MEDIA
    pub no_remote: bool,
}
