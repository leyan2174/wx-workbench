//! 原生文件/字节转录、显式媒体清单回写与只读数据库关联；不发现账号。
//!
//! 清单格式：{"entries":[{"username":"u","source":"message_0.db",
//! "local_id":1,"audio":"voice/1.silk"}]}。audio 必须相对显式媒体根目录。
//! source 必须与聊天 JSON 完全匹配；不从 local_id 推断分片。
//! 本地后端限制输出并通过 Job Object 回收已纳管子进程；未验证真实模型识别。
//! batch 自动固定账号快照并逐条提交成功缓存，聊天 JSON 最后原子发布。
//! Python Whisper 与 whisper.cpp 是两个显式、互不回退的本地后端。

pub mod batch;
pub mod cache;
pub mod cached;
pub mod prepared_audio;
pub mod receipt;
pub mod writeback;

use crate::infrastructure::audio::prepare_wav_bytes;
#[cfg(test)]
use crate::infrastructure::audio::{pcm24k_to_wav, validate_wav};
use crate::infrastructure::transcription::{local, local_python, openai};
use crate::service::operation_requests::asr::BackendId;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_AUDIO_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum Backend {
    WhisperCpp(local::LocalConfig),
    PythonWhisper(local_python::LocalPythonConfig),
    OpenAiCompatible {
        client: openai::OpenAiTranscriber,
        allow_upload: bool,
    },
}

impl Backend {
    pub fn identity(&self) -> BackendId {
        match self {
            Self::WhisperCpp(_) => BackendId::WhisperCpp,
            Self::PythonWhisper(_) => BackendId::PythonWhisper,
            Self::OpenAiCompatible { .. } => BackendId::OpenAiCompatible,
        }
    }

    /// 授权先于客户端创建；不读取环境配置或默认凭证。
    pub fn openai_compatible(config: openai::OpenAiConfig, allow_upload: bool) -> Result<Self> {
        ensure!(allow_upload, "audio upload requires explicit authorization");
        Ok(Self::OpenAiCompatible {
            client: openai::OpenAiTranscriber::new(config)?,
            allow_upload,
        })
    }

    fn check_authorization(&self) -> Result<()> {
        if let Self::OpenAiCompatible { allow_upload, .. } = self {
            ensure!(
                *allow_upload,
                "audio upload requires explicit authorization"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transcription {
    pub text: String,
    pub language: String,
    pub backend: String,
}

/// 阻塞 API；异步调用者应使用 spawn_blocking。
pub fn transcribe_audio(input: &Path, backend: &Backend) -> Result<Transcription> {
    backend.check_authorization()?;
    let wav = prepare_wav(input)?;
    transcribe_wav(&wav, backend)
}

/// 直接处理已授权来源的音频字节；不创建临时 SILK，不从环境推断后端。
/// 授权先于格式检查和解码；本地引擎仍复用现有受控临时 WAV 流程。
pub fn transcribe_audio_bytes(bytes: &[u8], backend: &Backend) -> Result<Transcription> {
    backend.check_authorization()?;
    let wav = prepare_wav_bytes(bytes)?;
    transcribe_wav(&wav, backend)
}

// 仅接收两个已授权入口完成校验/解码的 WAV，共用后端逻辑而不重复解码。
fn transcribe_wav(wav: &[u8], backend: &Backend) -> Result<Transcription> {
    let mut result = match backend {
        Backend::PythonWhisper(config) => {
            let result = local_python::transcribe_wav(config, wav)?;
            Ok::<Transcription, anyhow::Error>(Transcription {
                text: result.text,
                language: result.language,
                backend: "python_whisper".into(),
            })
        }
        Backend::WhisperCpp(config) => {
            let mut builder = tempfile::Builder::new();
            builder.prefix("wx-asr-audio-").suffix(".wav");
            let mut audio = match &config.temp_root {
                Some(root) => builder.tempfile_in(root),
                None => builder.tempfile(),
            }?;
            audio.write_all(wav)?;
            audio.flush()?;
            let result = local::transcribe(config, audio.path())?;
            Ok(Transcription {
                text: result.text,
                language: result.language,
                backend: result.backend,
            })
        }
        Backend::OpenAiCompatible {
            client,
            allow_upload,
        } => {
            let result = client.transcribe_wav(wav, *allow_upload)?;
            Ok(Transcription {
                text: result.text,
                language: result.language,
                backend: "openai_compatible".into(),
            })
        }
    }?;
    result.backend = backend.identity().as_str().into();
    Ok(result)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = File::open(path).context("open explicit input file")?;
    ensure!(file.metadata()?.is_file(), "input must be a regular file");
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "input exceeds {limit} byte limit");
    Ok(bytes)
}

/// 根据文件头识别，不依赖扩展名。WAV 仅接受 PCM16 单声道，不偷偷转码。
pub fn prepare_wav(input: &Path) -> Result<Vec<u8>> {
    let bytes = read_bounded(input, MAX_AUDIO_BYTES)?;
    prepare_wav_bytes(&bytes)
}

/// 只保存已经检查过的完整身份映射；不支持模糊来源或数据库查找。
#[derive(Debug)]
pub struct OfflineMedia {
    root: PathBuf,
    entries: BTreeMap<(String, String, i64), PathBuf>,
}

impl OfflineMedia {
    pub fn from_manifest(manifest: &Path, media_root: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Manifest {
            entries: Vec<Entry>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Entry {
            username: String,
            source: String,
            local_id: i64,
            audio: PathBuf,
        }
        let manifest: Manifest = serde_json::from_slice(&read_bounded(manifest, 4 * 1024 * 1024)?)
            .context("invalid offline media manifest")?;
        let root = fs::canonicalize(media_root).context("resolve explicit media root")?;
        ensure!(root.is_dir(), "media root is not a directory");
        let mut entries = BTreeMap::new();
        for entry in manifest.entries {
            ensure!(
                !entry.username.trim().is_empty()
                    && !entry.source.trim().is_empty()
                    && !entry.source.eq_ignore_ascii_case("unknown")
                    && entry.local_id > 0,
                "media entry requires explicit username/source/positive local_id"
            );
            ensure!(
                !entry.audio.as_os_str().is_empty()
                    && entry
                        .audio
                        .components()
                        .all(|c| matches!(c, Component::Normal(_)))
                    && !entry.audio.to_string_lossy().contains(':'),
                "audio path must be relative without traversal or stream suffix"
            );
            let path = root.join(entry.audio);
            let resolved = fs::canonicalize(&path).context("resolve manifest audio")?;
            ensure!(
                resolved.starts_with(&root) && resolved.is_file(),
                "audio escapes media root or is not a file"
            );
            ensure!(
                entries
                    .insert((entry.username, entry.source, entry.local_id), path)
                    .is_none(),
                "ambiguous duplicate media identity"
            );
        }
        Ok(Self { root, entries })
    }

    pub fn resolve(&self, identity: &writeback::VoiceIdentity) -> Result<PathBuf> {
        let path = self
            .entries
            .get(&(
                identity.username.clone(),
                identity.source.clone(),
                identity.local_id,
            ))
            .context("unknown media identity; explicit username/source/local_id match required")?;
        // 再次解析链接，避免清单加载后路径被换成媒体根之外的链接。
        let resolved = fs::canonicalize(path)?;
        ensure!(
            resolved.starts_with(&self.root) && resolved.is_file(),
            "audio escapes media root or is not a file"
        );
        Ok(resolved)
    }
}

pub fn transcribe_chat(
    input: &Path,
    output: &Path,
    media: &OfflineMedia,
    backend: &Backend,
) -> Result<writeback::WritebackReport> {
    backend.check_authorization()?;
    writeback::transcribe_file(input, output, |identity| {
        let audio = media.resolve(identity)?;
        Ok(transcribe_audio(&audio, backend)?.text)
    })
}

#[cfg(test)]
mod pipeline_tests;

#[cfg(test)]
mod byte_pipeline_tests {
    use super::*;

    #[test]
    fn file_and_byte_conversion_are_identical_for_wav_and_silk() {
        let dir = tempfile::tempdir().unwrap();
        let wav = pcm24k_to_wav(&[0, 0, 127, 0]).unwrap();
        let silk = include_bytes!("../../../tests/fixtures/audio/silence.silk");
        for (name, bytes) in [("wav", wav.as_slice()), ("silk", silk.as_slice())] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                prepare_wav(&path).unwrap(),
                prepare_wav_bytes(bytes).unwrap()
            );
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        let before = fs::read_dir(dir.path()).unwrap().count();
        for invalid in [
            &b""[..],
            &b"not audio"[..],
            &b"RIFFbad"[..],
            &b"#!SILK_V3"[..],
        ] {
            assert!(prepare_wav_bytes(invalid).is_err());
        }
        assert_eq!(before, fs::read_dir(dir.path()).unwrap().count());
    }

    #[test]
    fn byte_input_limit_is_checked_before_decode_or_backend() {
        let oversized = vec![0u8; MAX_AUDIO_BYTES + 1];
        let backend = Backend::WhisperCpp(local::LocalConfig::new(
            "missing.exe".into(),
            "missing.bin".into(),
        ));
        assert!(prepare_wav_bytes(&oversized)
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
        assert!(transcribe_audio_bytes(&oversized, &backend)
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
    }

    #[test]
    fn forged_cloud_backend_is_denied_before_bytes_or_path_access() {
        let client = openai::OpenAiTranscriber::new(openai::OpenAiConfig {
            base_url: "http://127.0.0.1:9/v1".into(),
            model: "synthetic".into(),
            language: None,
            api_key: "synthetic-not-a-secret".into(),
            timeout: std::time::Duration::from_secs(1),
            max_audio_bytes: 1024,
        })
        .unwrap();
        let backend = Backend::OpenAiCompatible {
            client,
            allow_upload: false,
        };
        assert!(transcribe_audio_bytes(b"invalid audio", &backend)
            .unwrap_err()
            .to_string()
            .contains("authorization"));
        assert!(transcribe_audio(Path::new("nonexistent-audio"), &backend)
            .unwrap_err()
            .to_string()
            .contains("authorization"));
    }
}
