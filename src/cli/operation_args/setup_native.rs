use std::path::PathBuf;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Backend {
    #[value(name = "python_whisper")]
    PythonWhisper,
    #[value(name = "whisper_cpp")]
    WhisperCpp,
    #[value(name = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(Default, Debug, clap::Args, Clone)]
pub struct Args {
    #[arg(long, conflicts_with_all = ["apply", "dry_run", "interactive", "db_dir", "backend", "whisper_binary", "whisper_model", "local_model", "openai_key_env"])]
    pub check: bool,
    /// 显式配置文件；缺省复用 WX_CLI_CONFIG 和现有配置定位逻辑
    #[arg(long)]
    pub config_path: Option<PathBuf>,
    /// 明确选择 db_storage 或含 db_storage 的账号目录
    #[arg(long)]
    pub db_dir: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub backend: Option<Backend>,
    #[arg(long)]
    pub whisper_binary: Option<PathBuf>,
    #[arg(long)]
    pub whisper_model: Option<PathBuf>,
    #[arg(long)]
    pub local_model: Option<String>,
    /// 仅保存凭据环境变量名；能力检查只报告是否已设置，不保存或回显其值
    #[arg(long)]
    pub openai_key_env: Option<String>,
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

impl From<Backend> for crate::service::operation_requests::setup_native::Backend {
    fn from(value: Backend) -> Self {
        match value {
            Backend::PythonWhisper => Self::PythonWhisper,
            Backend::WhisperCpp => Self::WhisperCpp,
            Backend::OpenAiCompatible => Self::OpenAiCompatible,
        }
    }
}

impl From<crate::service::operation_requests::setup_native::Backend> for Backend {
    fn from(value: crate::service::operation_requests::setup_native::Backend) -> Self {
        match value {
            crate::service::operation_requests::setup_native::Backend::PythonWhisper => {
                Self::PythonWhisper
            }
            crate::service::operation_requests::setup_native::Backend::WhisperCpp => {
                Self::WhisperCpp
            }
            crate::service::operation_requests::setup_native::Backend::OpenAiCompatible => {
                Self::OpenAiCompatible
            }
        }
    }
}

impl From<Args> for crate::service::operation_requests::setup_native::Args {
    fn from(value: Args) -> Self {
        Self {
            check: value.check,
            config_path: value.config_path,
            db_dir: value.db_dir,
            backend: value.backend.map(Into::into),
            whisper_binary: value.whisper_binary,
            whisper_model: value.whisper_model,
            local_model: value.local_model,
            openai_key_env: value.openai_key_env,
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
            backend: value.backend.map(Into::into),
            whisper_binary: value.whisper_binary,
            whisper_model: value.whisper_model,
            local_model: value.local_model,
            openai_key_env: value.openai_key_env,
            interactive: value.interactive,
            dry_run: value.dry_run,
            apply: value.apply,
            yes: value.yes,
        }
    }
}
