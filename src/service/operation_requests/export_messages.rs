use std::path::PathBuf;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Args {
    /// 输出根目录；省略时读取当前账号配置的 output_base_dir
    pub output_dir: Option<PathBuf>,
    /// 精确 username 或目录中的 unknown_<完整表哈希>，逗号分隔；支持 WECHAT_EXPORT_CONTACTS
    pub contacts: Option<String>,
    /// csv,html,json，默认全部；也支持 WECHAT_EXPORT_FORMATS
    pub formats: Option<String>,
    /// 禁用媒体复制与解码，仍保留显式 disabled 标记
    pub no_media: bool,
    /// 更新本工具已绑定的目录，不认领未知 legacy 目录
    pub update: bool,
    /// 只列出精确联系人与目标路径
    pub dry_run: bool,
    /// 有媒体缺失或失败时仍返回成功；诊断始终保留
    pub allow_missing_media: bool,
    /// 单个附件最大字节数，默认 64 MiB
    pub max_media_bytes: Option<u64>,
    /// 每个聊天累计媒体最大字节数，默认 2 GiB
    pub max_total_media_bytes: Option<u64>,
}
