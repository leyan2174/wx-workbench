use std::path::PathBuf;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Mode {
    Status,
    Plan,
}

#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    /// 默认输出只读 JSON 状态/计划，不启动交互式删除。
    #[arg(value_enum, conflicts_with = "execute")]
    pub mode: Option<Mode>,

    /// 配置路径；省略时使用程序统一的配置查找规则，只选择一次。
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// 覆盖程序默认 runtime 根目录，通常无需指定。
    #[arg(long)]
    pub runtime_root: Option<PathBuf>,

    /// 只生成 JSON 计划；不创建 runtime 目录、锁或缓存。
    #[arg(long, conflicts_with = "execute")]
    pub dry_run: bool,

    /// 已绑定当前账号的 _directory_export.json 或 _cleanup_inventory.json。
    #[arg(long = "native-inventory", conflicts_with = "execute")]
    pub native_inventories: Vec<PathBuf>,

    /// 逐文件旧产物接管 JSON，必须同时明确授权并确认当前账号。
    #[arg(long, requires_all = ["authorize_legacy", "confirm_account"], conflicts_with = "execute")]
    pub adoption_manifest: Option<PathBuf>,

    /// 本次明确授权读取/清理接管清单中的旧文件；不能解除密钥或输入保护。
    #[arg(long, requires = "confirm_account")]
    pub authorize_legacy: bool,

    /// 单独授权当前配置的 keys_file；生成计划和执行时都必须明确提供。
    #[arg(long, requires = "confirm_account", conflicts_with_all = ["authorize_legacy", "native_inventories", "adoption_manifest"])]
    pub authorize_key_removal: bool,

    /// 逐字确认所选 runtime ID；不是微信昵称，也不接受前缀匹配。
    #[arg(long)]
    pub confirm_account: Option<String>,

    /// 将计划写入一个不存在的绝对 JSON 路径；不覆盖，也不创建父目录。
    #[arg(long, conflicts_with = "execute")]
    pub write_plan: Option<PathBuf>,

    /// 显式执行先前计划；必须同时指定 plan、select 和 confirm-account。
    #[arg(long, requires_all = ["plan", "select", "confirm_account"], conflicts_with_all = ["dry_run", "mode", "write_plan", "native_inventories", "adoption_manifest"])]
    pub execute: bool,

    /// 已审阅的 JSON 计划绝对路径；内容、来源、目录身份和文件都会重新核验。
    #[arg(long, requires = "execute")]
    pub plan: Option<PathBuf>,

    /// 精确文件 ID，可重复或逗号分隔；禁止 all、分类、序号、短 ID 和通配符。
    #[arg(long, value_delimiter = ',', requires = "execute")]
    pub select: Vec<String>,
}

impl From<Mode> for crate::service::operation_requests::cleanup_native::Mode {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Status => Self::Status,
            Mode::Plan => Self::Plan,
        }
    }
}

impl From<crate::service::operation_requests::cleanup_native::Mode> for Mode {
    fn from(value: crate::service::operation_requests::cleanup_native::Mode) -> Self {
        match value {
            crate::service::operation_requests::cleanup_native::Mode::Status => Self::Status,
            crate::service::operation_requests::cleanup_native::Mode::Plan => Self::Plan,
        }
    }
}

impl From<Args> for crate::service::operation_requests::cleanup_native::Args {
    fn from(value: Args) -> Self {
        Self {
            mode: value.mode.map(Into::into),
            config: value.config,
            runtime_root: value.runtime_root,
            dry_run: value.dry_run,
            native_inventories: value.native_inventories,
            adoption_manifest: value.adoption_manifest,
            authorize_legacy: value.authorize_legacy,
            authorize_key_removal: value.authorize_key_removal,
            confirm_account: value.confirm_account,
            write_plan: value.write_plan,
            execute: value.execute,
            plan: value.plan,
            select: value.select,
        }
    }
}

impl From<crate::service::operation_requests::cleanup_native::Args> for Args {
    fn from(value: crate::service::operation_requests::cleanup_native::Args) -> Self {
        Self {
            mode: value.mode.map(Into::into),
            config: value.config,
            runtime_root: value.runtime_root,
            dry_run: value.dry_run,
            native_inventories: value.native_inventories,
            adoption_manifest: value.adoption_manifest,
            authorize_legacy: value.authorize_legacy,
            authorize_key_removal: value.authorize_key_removal,
            confirm_account: value.confirm_account,
            write_plan: value.write_plan,
            execute: value.execute,
            plan: value.plan,
            select: value.select,
        }
    }
}
