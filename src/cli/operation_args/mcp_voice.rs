use super::asr::BackendArgs;
use std::path::PathBuf;

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    #[command(flatten)]
    pub backend: BackendArgs,
    /// 显式主机转录缓存文件；省略时不持久化转录缓存。
    #[arg(long)]
    pub voice_cache_file: Option<PathBuf>,
}

impl From<Args> for crate::service::mcp::VoiceSettings {
    fn from(value: Args) -> Self {
        Self {
            backend: value.backend.into(),
            voice_cache_file: value.voice_cache_file,
        }
    }
}

impl From<crate::service::mcp::VoiceSettings> for Args {
    fn from(value: crate::service::mcp::VoiceSettings) -> Self {
        Self {
            backend: value.backend.into(),
            voice_cache_file: value.voice_cache_file,
        }
    }
}
