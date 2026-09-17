use std::path::PathBuf;

#[derive(Default, Debug, clap::Args, Clone)]
pub struct Args {
    #[arg(long, conflicts_with_all = ["apply", "dry_run", "interactive", "db_dir"])]
    pub check: bool,
    /// 显式配置文件；缺省复用 WX_CLI_CONFIG 和现有配置定位逻辑
    #[arg(long)]
    pub config_path: Option<PathBuf>,
    /// 明确选择 db_storage 或含 db_storage 的账号目录
    #[arg(long)]
    pub db_dir: Option<PathBuf>,
    #[arg(long)]
    pub interactive: bool,
    #[arg(long, conflicts_with = "apply")]
    pub dry_run: bool,
    #[arg(long)]
    pub apply: bool,
    /// 明确确认写入；非交互 apply 必须同时提供
    #[arg(long, requires = "apply")]
    pub yes: bool,
}

impl From<Args> for crate::service::operation_requests::setup_native::Args {
    fn from(value: Args) -> Self {
        Self {
            check: value.check,
            config_path: value.config_path,
            db_dir: value.db_dir,
            interactive: value.interactive,
            dry_run: value.dry_run,
            apply: value.apply,
            yes: value.yes,
        }
    }
}

impl From<crate::service::operation_requests::setup_native::Args> for Args {
    fn from(value: crate::service::operation_requests::setup_native::Args) -> Self {
        Self {
            check: value.check,
            config_path: value.config_path,
            db_dir: value.db_dir,
            interactive: value.interactive,
            dry_run: value.dry_run,
            apply: value.apply,
            yes: value.yes,
        }
    }
}
