use crate::toolkit::asr::backend::{BackendId, Entry, Options};
use anyhow::{ensure, Result};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum BackendKind {
    #[serde(alias = "local")]
    Local,
    #[serde(rename = "whisper_cpp")]
    WhisperCpp,
    #[serde(rename = "python_whisper")]
    PythonWhisper,
    #[serde(
        rename = "openai_compatible",
        alias = "ExplicitOpenAi",
        alias = "explicit-open-ai",
        alias = "openai"
    )]
    ExplicitOpenAi,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendArgs {
    pub backend: BackendKind,
    /// 显式 whisper.cpp 可执行文件路径
    pub whisper_binary: Option<PathBuf>,
    /// 显式本地模型路径；不自动下载
    pub whisper_model: Option<PathBuf>,
    pub language: String,
    pub threads: Option<usize>,
    pub timeout_seconds: u64,
    /// 明确允许本次单文件或整个聊天批次上传音频
    pub allow_upload: bool,
    pub openai_base_url: Option<String>,
    pub openai_model: Option<String>,
    /// 显式 UTF-8 凭证文件；不读取环境变量或默认密钥
    pub api_key_file: Option<PathBuf>,
    /// 本地后端临时 WAV 和结果文件根目录
    pub temp_root: Option<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TranscribeAudioNativeArgs {
    /// 单个 SILK_V3 或 PCM16 单声道 WAV 文件
    pub input: PathBuf,
    pub backend: BackendArgs,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TranscribeChatNativeArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 完整 username/source/local_id 到相对音频路径的 JSON 清单
    pub media_manifest: PathBuf,
    pub media_root: PathBuf,
    pub backend: BackendArgs,
}

impl Default for BackendArgs {
    fn default() -> Self {
        Self {
            backend: BackendKind::Local,
            whisper_binary: None,
            whisper_model: None,
            language: "auto".into(),
            threads: None,
            timeout_seconds: 120,
            allow_upload: false,
            openai_base_url: None,
            openai_model: None,
            api_key_file: None,
            temp_root: None,
        }
    }
}

impl BackendKind {
    pub fn identity(self, entry: Entry) -> BackendId {
        let name = match self {
            Self::Local => "local",
            Self::WhisperCpp => "whisper_cpp",
            Self::PythonWhisper => "python_whisper",
            Self::ExplicitOpenAi => "openai_compatible",
        };
        BackendId::parse(name, entry).expect("known backend name")
    }
}
impl BackendArgs {
    pub fn validate_for(&self, backend: BackendId) -> Result<()> {
        Options {
            cloud: self.openai_base_url.is_some()
                || self.openai_model.is_some()
                || self.api_key_file.is_some(),
            cpp_paths: self.whisper_binary.is_some() || self.whisper_model.is_some(),
            threads: self.threads,
            temp_root: self.temp_root.is_some(),
            allow_upload: self.allow_upload,
        }
        .validate(backend, &self.language, self.timeout_seconds)
    }

    /// Pure preflight, before host paths, credentials or audio are read.
    pub fn validate_explicit(&self) -> Result<BackendId> {
        let id = self.backend.identity(Entry::Native);
        self.validate_for(id)?;
        match id {
            BackendId::WhisperCpp => ensure!(
                self.whisper_binary.is_some() && self.whisper_model.is_some(),
                "--whisper-binary and --whisper-model are required"
            ),
            BackendId::OpenAiCompatible => ensure!(
                self.openai_base_url.is_some()
                    && self.openai_model.is_some()
                    && self.api_key_file.is_some(),
                "--openai-base-url, --openai-model and --api-key-file are required"
            ),
            BackendId::PythonWhisper => {
                anyhow::bail!("python_whisper requires a configured batch or configured MCP entry")
            }
        }
        Ok(id)
    }
}
