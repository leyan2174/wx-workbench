//! 显式启用的 OpenAI 兼容音频转录；不负责配置读取、解码或缓存回写。
use reqwest::blocking::{multipart, Client};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::{redirect::Policy, Url};
use serde_json::Value;
use std::{fmt, io::Read, time::Duration};

pub const OPENAI_AUDIO_LIMIT_BYTES: usize = 25 * 1024 * 1024;
const RESPONSE_LIMIT_BYTES: u64 = 1024 * 1024;

/// 无默认配置；必须由上层根据用户明确选择创建。
pub struct OpenAiConfig {
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub api_key: String,
    pub timeout: Duration,
    pub max_audio_bytes: usize,
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // URL、模型等也可能被误填为凭证，全部隐藏。
        f.write_str("OpenAiConfig { fields: [REDACTED] }")
    }
}

pub struct OpenAiTranscriber {
    client: Client,
    endpoint: Url,
    authorization: HeaderValue,
    model: String,
    language: Option<String>,
    max_audio_bytes: usize,
    timeout: Duration,
}

impl fmt::Debug for OpenAiTranscriber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpenAiTranscriber { fields: [REDACTED] }")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Transcription {
    pub text: String,
    pub language: String,
}

impl fmt::Debug for Transcription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Transcription { content: [REDACTED] }")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiError {
    UploadNotAuthorized,
    InvalidConfig,
    EmptyAudio,
    AudioTooLarge,
    Timeout,
    Transport,
    Http { status: u16, json_error: bool },
    ResponseTooLarge,
    InvalidResponse,
}

impl fmt::Display for OpenAiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UploadNotAuthorized => f.write_str("Audio upload was not explicitly authorized"),
            Self::InvalidConfig => f.write_str("Invalid OpenAI transcription configuration"),
            Self::EmptyAudio => f.write_str("Audio is empty"),
            Self::AudioTooLarge => f.write_str("Audio exceeds configured upload limit"),
            Self::Timeout => f.write_str("Audio transcription timed out"),
            Self::Transport => f.write_str("Audio transcription transport failed"),
            Self::Http { status: 401, .. } => f.write_str("OpenAI authentication failed (401)"),
            Self::Http { status: 429, .. } => f.write_str("OpenAI rate limited (429)"),
            Self::Http { status, .. } => write!(f, "Audio transcription HTTP error ({status})"),
            Self::ResponseTooLarge => f.write_str("Transcription response exceeds size limit"),
            Self::InvalidResponse => f.write_str("Invalid transcription JSON response"),
        }
    }
}

impl std::error::Error for OpenAiError {}

impl OpenAiTranscriber {
    /// 仅供同模块缓存区分非秘密配置；绝不返回认证头或客户端状态。
    pub(crate) fn cache_identity(&self) -> (&str, &str, Option<&str>, usize) {
        (
            self.endpoint.as_str(),
            &self.model,
            self.language.as_deref(),
            self.max_audio_bytes,
        )
    }

    pub fn new(config: OpenAiConfig) -> Result<Self, OpenAiError> {
        let mut endpoint = Url::parse(&config.base_url).map_err(|_| OpenAiError::InvalidConfig)?;
        let loopback = matches!(endpoint.host_str(), Some("127.0.0.1" | "[::1]"));
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || config.model.trim().is_empty()
            || config.api_key.trim().is_empty()
            || config
                .language
                .as_ref()
                .is_some_and(|v| v.trim().is_empty())
            || config.timeout.is_zero()
            || config.max_audio_bytes == 0
            || config.max_audio_bytes > OPENAI_AUDIO_LIMIT_BYTES
        {
            return Err(OpenAiError::InvalidConfig);
        }
        let path = format!(
            "{}/audio/transcriptions",
            endpoint.path().trim_end_matches('/')
        );
        endpoint.set_path(&path);
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", config.api_key))
            .map_err(|_| OpenAiError::InvalidConfig)?;
        authorization.set_sensitive(true);
        let client = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.timeout)
            .redirect(Policy::none())
            // 不读取系统代理环境变量；禁止代理意外接收音频。
            .no_proxy()
            .build()
            .map_err(|_| OpenAiError::Transport)?;
        Ok(Self {
            client,
            endpoint,
            authorization,
            model: config.model,
            language: config.language,
            max_audio_bytes: config.max_audio_bytes,
            timeout: config.timeout,
        })
    }

    /// 仅收紧当前请求预算，不重读凭证、不改变成功缓存身份，也不允许恢复更长时间。
    pub fn tighten_timeout(&mut self, remaining: Duration) -> Result<(), OpenAiError> {
        if remaining.is_zero() {
            return Err(OpenAiError::Timeout);
        }
        self.timeout = self.timeout.min(remaining);
        Ok(())
    }

    /// 仅接收调用者已解码的 WAV 字节，不读取文件。每次上传必须明确授权。
    pub fn transcribe_wav(
        &self,
        audio: &[u8],
        upload_authorized: bool,
    ) -> Result<Transcription, OpenAiError> {
        if !upload_authorized {
            return Err(OpenAiError::UploadNotAuthorized);
        }
        if audio.is_empty() {
            return Err(OpenAiError::EmptyAudio);
        }
        if audio.len() > self.max_audio_bytes {
            return Err(OpenAiError::AudioTooLarge);
        }
        let part = multipart::Part::bytes(audio.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|_| OpenAiError::InvalidConfig)?;
        let mut form = multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .part("file", part);
        if let Some(language) = &self.language {
            form = form.text("language", language.clone());
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, self.authorization.clone())
            .multipart(form)
            .timeout(self.timeout)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    OpenAiError::Timeout
                } else {
                    OpenAiError::Transport
                }
            })?;
        let status = response.status();
        let mut body = Vec::new();
        // take 防止缺失或伪造 Content-Length 的响应无限增长。
        response
            .take(RESPONSE_LIMIT_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|e| {
                let timed_out = e.kind() == std::io::ErrorKind::TimedOut
                    || e.get_ref()
                        .and_then(|e| e.downcast_ref::<reqwest::Error>())
                        .is_some_and(reqwest::Error::is_timeout);
                if timed_out {
                    OpenAiError::Timeout
                } else {
                    OpenAiError::Transport
                }
            })?;
        if body.len() as u64 > RESPONSE_LIMIT_BYTES {
            return Err(OpenAiError::ResponseTooLarge);
        }
        let parsed = serde_json::from_slice::<Value>(&body);
        if !status.is_success() {
            return Err(OpenAiError::Http {
                status: status.as_u16(),
                // 仅标识 JSON 错误结构，绝不转发可包含 key/音频的正文。
                json_error: parsed.as_ref().is_ok_and(|v| v.get("error").is_some()),
            });
        }
        let value = parsed.map_err(|_| OpenAiError::InvalidResponse)?;
        let text = match value.get("text") {
            Some(Value::String(text)) => text.trim().to_owned(),
            Some(Value::Null) => String::new(),
            _ => return Err(OpenAiError::InvalidResponse),
        };
        let language = match value.get("language") {
            None | Some(Value::Null) => "unknown".to_owned(),
            Some(Value::String(language)) => language.clone(),
            _ => return Err(OpenAiError::InvalidResponse),
        };
        Ok(Transcription { text, language })
    }
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod tests;
