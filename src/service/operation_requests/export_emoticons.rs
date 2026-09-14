use std::path::PathBuf;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 输出目录；默认配置文件旁的 exported_emoticons
    pub output_dir: Option<PathBuf>,
    /// 只列出表情，不下载，不创建输出目录
    pub dry_run: bool,
    /// 按描述或表情包 product_id 过滤，忽略大小写
    pub filter: Option<String>,
}
