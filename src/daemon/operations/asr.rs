//! 显式后端参数及单文件、聊天转录入口；不自动读取账号和云端凭证。
use crate::toolkit::asr::{self, local, openai, Backend, OfflineMedia};
use anyhow::{ensure, Context, Result};
use clap::{Args, ValueEnum};
use std::{fs::File, io::Read, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum BackendKind {
    Local,
    ExplicitOpenAi,
}

#[derive(Args, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendArgs {
    #[arg(long, value_enum, default_value = "local")]
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

impl BackendArgs {
    pub fn build(self) -> Result<Backend> {
        ensure!(self.timeout_seconds > 0, "timeout must be positive");
        ensure!(
            !self.language.trim().is_empty(),
            "language must not be empty"
        );
        match self.backend {
            BackendKind::Local => {
                ensure!(
                    !self.allow_upload
                        && self.openai_base_url.is_none()
                        && self.openai_model.is_none()
                        && self.api_key_file.is_none(),
                    "cloud options require explicit-open-ai backend"
                );
                let mut config = local::LocalConfig::new(
                    self.whisper_binary
                        .context("--whisper-binary is required")?,
                    self.whisper_model.context("--whisper-model is required")?,
                );
                config.language = self.language;
                if let Some(threads) = self.threads {
                    ensure!(threads > 0, "threads must be positive");
                    config.threads = threads;
                }
                config.timeout = Duration::from_secs(self.timeout_seconds);
                config.output_format = local::OutputFormat::Json;
                config.temp_root = self.temp_root;
                Ok(Backend::Local(config))
            }
            BackendKind::ExplicitOpenAi => {
                ensure!(
                    self.allow_upload,
                    "--allow-upload is required before reading credentials or audio"
                );
                ensure!(
                    self.whisper_binary.is_none()
                        && self.whisper_model.is_none()
                        && self.threads.is_none()
                        && self.temp_root.is_none(),
                    "local options cannot be used with explicit-open-ai"
                );
                let base_url = self
                    .openai_base_url
                    .context("--openai-base-url is required")?;
                let model = self.openai_model.context("--openai-model is required")?;
                let key_file = self.api_key_file.context("--api-key-file is required")?;
                let mut key = zeroize::Zeroizing::new(String::new());
                File::open(key_file)
                    .context("open explicit API key file")?
                    .take(16_385)
                    .read_to_string(&mut key)
                    .context("read explicit UTF-8 API key file")?;
                ensure!(key.len() <= 16_384, "API key file exceeds limit");
                Backend::explicit_openai(
                    openai::OpenAiConfig {
                        base_url,
                        model,
                        language: (self.language != "auto").then_some(self.language),
                        api_key: key.trim().to_owned(),
                        timeout: Duration::from_secs(self.timeout_seconds),
                        max_audio_bytes: openai::OPENAI_AUDIO_LIMIT_BYTES,
                    },
                    true,
                )
            }
        }
    }
}

#[derive(Args, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TranscribeAudioNativeArgs {
    /// 单个 SILK_V3 或 PCM16 单声道 WAV 文件
    pub input: PathBuf,
    #[command(flatten)]
    pub backend: BackendArgs,
}

#[derive(Args, serde::Serialize, serde::Deserialize, Clone, Debug)]
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

pub fn cmd_transcribe_audio_native(args: TranscribeAudioNativeArgs) -> Result<()> {
    let backend = args.backend.build()?;
    let result = asr::transcribe_audio(&args.input, &backend)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn cmd_transcribe_chat_native(args: TranscribeChatNativeArgs) -> Result<()> {
    let backend = args.backend.build()?;
    let media = OfflineMedia::from_manifest(&args.media_manifest, &args.media_root)?;
    let report = asr::transcribe_chat(&args.input, &args.output, &media, &backend)?;
    let errors: Vec<_> = report
        .errors
        .iter()
        .map(|e| serde_json::json!({"index": e.index, "error": e.error}))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "transcribed": report.transcribed, "skipped_existing": report.skipped_existing,
            "skipped_non_voice": report.skipped_non_voice, "failed": report.failed, "errors": errors
        }))?
    );
    ensure!(
        report.failed == 0,
        "{} voice messages failed; successful results were saved",
        report.failed
    );
    Ok(())
}
