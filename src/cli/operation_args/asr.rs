use std::path::PathBuf;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum BackendKind {
    #[value(name = "whisper_cpp")]
    WhisperCpp,
    #[value(name = "python_whisper")]
    PythonWhisper,
    #[value(name = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(clap::Args, Debug, Clone)]
pub struct BackendArgs {
    #[arg(long, value_enum, default_value = "whisper_cpp")]
    pub backend: BackendKind,
    /// 显式 whisper.cpp 可执行文件路径
    #[arg(long)]
    pub whisper_binary: Option<PathBuf>,
    /// 显式本地模型路径；不自动下载
    #[arg(long)]
    pub whisper_model: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    pub language: String,
    #[arg(long)]
    pub threads: Option<usize>,
    #[arg(long, default_value_t = 120)]
    pub timeout_seconds: u64,
    /// 明确允许本次单文件或整个聊天批次上传音频
    #[arg(long)]
    pub allow_upload: bool,
    #[arg(long)]
    pub openai_base_url: Option<String>,
    #[arg(long)]
    pub openai_model: Option<String>,
    /// 显式 UTF-8 凭证文件；不读取环境变量或默认密钥
    #[arg(long)]
    pub api_key_file: Option<PathBuf>,
    /// 本地后端临时 WAV 和结果文件根目录
    #[arg(long)]
    pub temp_root: Option<PathBuf>,
}

#[derive(clap::Args, Clone, Debug)]
pub struct TranscribeAudioNativeArgs {
    /// 单个 SILK_V3 或 PCM16 单声道 WAV 文件
    pub input: PathBuf,
    #[command(flatten)]
    pub backend: BackendArgs,
}

#[derive(clap::Args, Clone, Debug)]
pub struct TranscribeChatNativeArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 完整 username/source/local_id 到相对音频路径的 JSON 清单
    #[arg(long)]
    pub media_manifest: PathBuf,
    #[arg(long)]
    pub media_root: PathBuf,
    #[command(flatten)]
    pub backend: BackendArgs,
}

impl From<BackendKind> for crate::service::operation_requests::asr::BackendKind {
    fn from(value: BackendKind) -> Self {
        match value {
            BackendKind::WhisperCpp => Self::WhisperCpp,
            BackendKind::PythonWhisper => Self::PythonWhisper,
            BackendKind::OpenAiCompatible => Self::OpenAiCompatible,
        }
    }
}

impl From<crate::service::operation_requests::asr::BackendKind> for BackendKind {
    fn from(value: crate::service::operation_requests::asr::BackendKind) -> Self {
        match value {
            crate::service::operation_requests::asr::BackendKind::WhisperCpp => Self::WhisperCpp,
            crate::service::operation_requests::asr::BackendKind::PythonWhisper => {
                Self::PythonWhisper
            }
            crate::service::operation_requests::asr::BackendKind::OpenAiCompatible => {
                Self::OpenAiCompatible
            }
        }
    }
}

impl From<BackendArgs> for crate::service::operation_requests::asr::BackendArgs {
    fn from(value: BackendArgs) -> Self {
        Self {
            backend: value.backend.into(),
            whisper_binary: value.whisper_binary,
            whisper_model: value.whisper_model,
            language: value.language,
            threads: value.threads,
            timeout_seconds: value.timeout_seconds,
            allow_upload: value.allow_upload,
            openai_base_url: value.openai_base_url,
            openai_model: value.openai_model,
            api_key_file: value.api_key_file,
            temp_root: value.temp_root,
        }
    }
}

impl From<crate::service::operation_requests::asr::BackendArgs> for BackendArgs {
    fn from(value: crate::service::operation_requests::asr::BackendArgs) -> Self {
        Self {
            backend: value.backend.into(),
            whisper_binary: value.whisper_binary,
            whisper_model: value.whisper_model,
            language: value.language,
            threads: value.threads,
            timeout_seconds: value.timeout_seconds,
            allow_upload: value.allow_upload,
            openai_base_url: value.openai_base_url,
            openai_model: value.openai_model,
            api_key_file: value.api_key_file,
            temp_root: value.temp_root,
        }
    }
}

impl From<TranscribeAudioNativeArgs>
    for crate::service::operation_requests::asr::TranscribeAudioNativeArgs
{
    fn from(value: TranscribeAudioNativeArgs) -> Self {
        Self {
            input: value.input,
            backend: value.backend.into(),
        }
    }
}

impl From<crate::service::operation_requests::asr::TranscribeAudioNativeArgs>
    for TranscribeAudioNativeArgs
{
    fn from(value: crate::service::operation_requests::asr::TranscribeAudioNativeArgs) -> Self {
        Self {
            input: value.input,
            backend: value.backend.into(),
        }
    }
}

impl From<TranscribeChatNativeArgs>
    for crate::service::operation_requests::asr::TranscribeChatNativeArgs
{
    fn from(value: TranscribeChatNativeArgs) -> Self {
        Self {
            input: value.input,
            output: value.output,
            media_manifest: value.media_manifest,
            media_root: value.media_root,
            backend: value.backend.into(),
        }
    }
}

impl From<crate::service::operation_requests::asr::TranscribeChatNativeArgs>
    for TranscribeChatNativeArgs
{
    fn from(value: crate::service::operation_requests::asr::TranscribeChatNativeArgs) -> Self {
        Self {
            input: value.input,
            output: value.output,
            media_manifest: value.media_manifest,
            media_root: value.media_root,
            backend: value.backend.into(),
        }
    }
}

impl Default for BackendArgs {
    fn default() -> Self {
        crate::service::operation_requests::asr::BackendArgs::default().into()
    }
}
