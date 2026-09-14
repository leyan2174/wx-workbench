use super::asr::BackendArgs;
use std::path::PathBuf;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub batch: BatchArgs,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BatchArgs {
    /// 使用显式原生后端参数，否则翻译固定账号 config.json。
    pub explicit_backend: bool,
    pub backend: BackendArgs,
    pub asr_cache_name: String,
}
