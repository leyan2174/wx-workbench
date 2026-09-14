use std::path::PathBuf;

#[derive(Debug, clap::Args, Clone)]
pub struct Args {
    /// 输出目录；默认配置文件旁的 exported_emoticons
    pub output_dir: Option<PathBuf>,
    /// 只列出表情，不下载，不创建输出目录
    #[arg(long)]
    pub dry_run: bool,
    /// 按描述或表情包 product_id 过滤，忽略大小写
    #[arg(long)]
    pub filter: Option<String>,
}

impl From<Args> for crate::service::operation_requests::export_emoticons::Args {
    fn from(value: Args) -> Self {
        Self {
            output_dir: value.output_dir,
            dry_run: value.dry_run,
            filter: value.filter,
        }
    }
}

impl From<crate::service::operation_requests::export_emoticons::Args> for Args {
    fn from(value: crate::service::operation_requests::export_emoticons::Args) -> Self {
        Self {
            output_dir: value.output_dir,
            dry_run: value.dry_run,
            filter: value.filter,
        }
    }
}
