use super::asr::BackendArgs;
use std::path::PathBuf;

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct BatchArgs {
    /// 使用显式原生后端参数，否则翻译固定账号 config.json。
    #[arg(long)]
    pub explicit_backend: bool,
    #[command(flatten)]
    pub backend: BackendArgs,
    #[arg(long, default_value = "batch-transcriptions.json")]
    pub asr_cache_name: String,
}

impl From<Args> for crate::service::operation_requests::asr_batch::Args {
    fn from(value: Args) -> Self {
        Self {
            input: value.input,
            output: value.output,
            batch: value.batch.into(),
        }
    }
}

impl From<crate::service::operation_requests::asr_batch::Args> for Args {
    fn from(value: crate::service::operation_requests::asr_batch::Args) -> Self {
        Self {
            input: value.input,
            output: value.output,
            batch: value.batch.into(),
        }
    }
}

impl From<BatchArgs> for crate::service::operation_requests::asr_batch::BatchArgs {
    fn from(value: BatchArgs) -> Self {
        Self {
            explicit_backend: value.explicit_backend,
            backend: value.backend.into(),
            asr_cache_name: value.asr_cache_name,
        }
    }
}

impl From<crate::service::operation_requests::asr_batch::BatchArgs> for BatchArgs {
    fn from(value: crate::service::operation_requests::asr_batch::BatchArgs) -> Self {
        Self {
            explicit_backend: value.explicit_backend,
            backend: value.backend.into(),
            asr_cache_name: value.asr_cache_name,
        }
    }
}
