use std::path::PathBuf;

#[derive(Debug, Default, clap::Args, Clone)]
pub struct Args {
    /// 输出基础目录；其下创建朋友圈图片，默认选中配置旁 wechat_files/<账号>
    #[arg(short = 'o', long)]
    pub output_dir: Option<PathBuf>,
    /// 显式认领未绑定来源的旧归档目录，已有来源冲突仍拒绝
    #[arg(long)]
    pub adopt_existing: bool,
}

impl From<Args> for crate::service::operation_requests::sns_archive::Args {
    fn from(value: Args) -> Self {
        Self {
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
        }
    }
}

impl From<crate::service::operation_requests::sns_archive::Args> for Args {
    fn from(value: crate::service::operation_requests::sns_archive::Args) -> Self {
        Self {
            output_dir: value.output_dir,
            adopt_existing: value.adopt_existing,
        }
    }
}
