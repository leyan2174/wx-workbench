//! 旧 MCP 成功转录缓存薄适配；目录须由调用者提供且可信，不发现配置或账号。
use super::{
    cache::{Cache, CacheKey, CachedTranscription, ConfigIdentity, StoreStatus},
    database_media::VoiceEvidence,
    receipt::{Proof, ReceiptState},
    Backend, Transcription,
};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub struct CachedRequest<'a> {
    pub cache_path: &'a Path,
    pub account: &'a str,
    pub username: &'a str,
    pub source: &'a str,
    pub local_id: i64,
    pub create_time: i64,
    pub silk: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheState {
    Hit,
    Stored,
    AlreadyPresent,
    ReadUnavailable,
    WriteUnavailable,
}

pub struct CachedOutcome {
    pub transcription: Transcription,
    pub create_time: i64,
    pub cache_state: CacheState,
}

pub struct ReceiptOutcome {
    pub cached: CachedOutcome,
    pub receipt: ReceiptState,
}

/// 命中成功空文本也直接返回；缓存失败不让成功识别变成失败。
pub fn transcribe_cached(request: &CachedRequest<'_>, backend: &Backend) -> Result<CachedOutcome> {
    Ok(transcribe(request, backend, None, None)?.0)
}

/// 宿主可复核最终响应及提交权限；普通缓存读写故障仍不丢弃识别结果。
#[cfg(test)]
pub fn transcribe_cached_checked(
    request: &CachedRequest<'_>,
    backend: &Backend,
    mut before_store: impl FnMut(&Transcription) -> Result<()>,
) -> Result<CachedOutcome> {
    Ok(transcribe(request, backend, None, Some(&mut before_store))?.0)
}

/// evidence 必须来自调用方绑定账号后的已验证音频；记录和索引同次原子发布。
/// receipt 故障不丢弃成功识别；宿主仍须在返回响应前复核账号、预算和取消状态。
pub fn transcribe_cached_with_receipt_checked(
    request: &CachedRequest<'_>,
    evidence: &VoiceEvidence,
    backend: &Backend,
    mut before_store: impl FnMut(&Transcription) -> Result<()>,
) -> Result<ReceiptOutcome> {
    let (cached, receipt) = transcribe(request, backend, Some(evidence), Some(&mut before_store))?;
    Ok(ReceiptOutcome {
        cached,
        receipt: receipt.expect("receipt requested"),
    })
}

type BeforeStore<'a> = &'a mut dyn FnMut(&Transcription) -> Result<()>;

fn transcribe(
    request: &CachedRequest<'_>,
    backend: &Backend,
    evidence: Option<&VoiceEvidence>,
    mut before_store: Option<BeforeStore<'_>>,
) -> Result<(CachedOutcome, Option<ReceiptState>)> {
    // 授权必须早于音频、模型与缓存访问；不得因已有云端结果绕过授权。
    backend.check_authorization()?;
    ensure!(
        request.silk.len() <= super::MAX_AUDIO_BYTES,
        "audio exceeds size limit"
    );
    ensure!(
        !request.account.trim().is_empty(),
        "explicit account required"
    );
    // 使用已有容器校验，命中时不重新解码；未命中复用唯一字节转录入口。
    crate::toolkit::audio::normalize_silk(request.silk)?;
    let proof = evidence.map(|e| Proof::new(request, e)).transpose()?;
    let (identity, _files, prepared) = identity(backend, request.create_time)?;
    let key = CacheKey::new(
        request.username,
        request.source,
        request.local_id,
        request.silk,
        &identity,
    )?;
    let cache = Cache::open(request.cache_path, request.account);
    let mut readable = false;
    if let Ok(cache) = &cache {
        match cache.lookup(&key) {
            Ok(Some(hit)) if hit.create_time == Some(request.create_time) => {
                let transcription = Transcription {
                    text: hit.text.clone(),
                    language: hit.language.clone(),
                    backend: backend_name(backend).into(),
                };
                let receipt = proof.as_ref().map(|proof| {
                    cache
                        .store_success_with_receipt_checked(&key, &hit, proof, &identity, || {
                            match &mut before_store {
                                Some(check) => check(&transcription),
                                None => Ok(()),
                            }
                        })
                        .map(|(_, state)| state)
                        .unwrap_or(ReceiptState::Unavailable)
                });
                return Ok((
                    CachedOutcome {
                        transcription,
                        create_time: request.create_time,
                        cache_state: CacheState::Hit,
                    },
                    receipt,
                ));
            }
            Ok(None) => readable = true,
            _ => {}
        }
    }
    let transcription =
        super::transcribe_audio_bytes(request.silk, prepared.as_ref().unwrap_or(backend))?;
    if let Some(check) = &mut before_store {
        check(&transcription)?;
    }
    let record = CachedTranscription {
        text: transcription.text.clone(),
        language: transcription.language.clone(),
        create_time: Some(request.create_time),
    };
    let mut receipt = proof.as_ref().map(|_| ReceiptState::Unavailable);
    let cache_state = if readable {
        let cache = cache.as_ref().expect("readable cache");
        let stored = if let Some(proof) = &proof {
            cache
                .store_success_with_receipt_checked(&key, &record, proof, &identity, || {
                    match &mut before_store {
                        Some(check) => check(&transcription),
                        None => Ok(()),
                    }
                })
                .map(|(status, state)| {
                    receipt = Some(state);
                    status
                })
        } else if let Some(check) = &mut before_store {
            cache.store_success_checked(&key, &record, || check(&transcription))
        } else {
            cache.store_success(&key, &record)
        };
        match stored {
            Ok(StoreStatus::Stored) => CacheState::Stored,
            Ok(StoreStatus::AlreadyPresent) => CacheState::AlreadyPresent,
            Err(_) => CacheState::WriteUnavailable,
        }
    } else {
        CacheState::ReadUnavailable
    };
    Ok((
        CachedOutcome {
            transcription,
            create_time: request.create_time,
            cache_state,
        },
        receipt,
    ))
}

pub(super) fn backend_name(backend: &Backend) -> &'static str {
    match backend {
        Backend::Local(_) => "whisper_cpp",
        Backend::LegacyPythonLocal(_) => "legacy-python-local",
        Backend::ExplicitOpenAi { .. } => "openai",
    }
}

pub(super) fn identity(
    backend: &Backend,
    create_time: i64,
) -> Result<(ConfigIdentity, Vec<File>, Option<Backend>)> {
    backend.check_authorization()?;
    match backend {
        Backend::LegacyPythonLocal(config) => {
            let engine = config.cache_identity()?;
            let options = serde_json::to_string(&("cached-byte-pipeline-v1", create_time))?;
            Ok((
                ConfigIdentity::new("legacy-python-local", &engine, "engine-defined", &options)?,
                Vec::new(),
                None,
            ))
        }
        Backend::Local(config) => {
            if let Some(root) = &config.temp_root {
                ensure!(
                    root.is_dir(),
                    "local temporary root must be an existing directory"
                );
            }
            ensure!(
                config.threads > 0 && !config.timeout.is_zero(),
                "invalid local backend configuration"
            );
            ensure!(
                !config.language.is_empty()
                    && config
                        .language
                        .bytes()
                        .all(|c| c.is_ascii_alphabetic() || c == b'-'),
                "invalid local language"
            );
            let mut config = config.clone();
            config.executable =
                std::fs::canonicalize(&config.executable).context("resolve explicit executable")?;
            config.model =
                std::fs::canonicalize(&config.model).context("resolve explicit model")?;
            let (program, program_file) = file_digest(&config.executable)?;
            let (model, model_file) = file_digest(&config.model)?;
            // 超时是执行预算，不改变已完成的识别结果；宿主收紧预算不能导致缓存失效。
            let options = serde_json::to_string(&(
                "cached-byte-pipeline-v1",
                program,
                config.executable.to_string_lossy(),
                config.threads,
                format!("{:?}", config.output_format),
                "--no-fallback",
                create_time,
            ))?;
            let identity = ConfigIdentity::new("whisper_cpp", &model, &config.language, &options)?;
            Ok((
                identity,
                vec![program_file, model_file],
                Some(Backend::Local(config)),
            ))
        }
        Backend::ExplicitOpenAi { client, .. } => {
            let (endpoint, model, language, max_audio_bytes) = client.cache_identity();
            // Option 序列化保留自动语言与显式语言的区别；凭据不参与缓存身份。
            let language = serde_json::to_string(&language)?;
            let options = serde_json::to_string(&(
                "cached-byte-pipeline-v1",
                endpoint,
                max_audio_bytes,
                create_time,
            ))?;
            Ok((
                ConfigIdentity::new("openai", model, &language, &options)?,
                Vec::new(),
                None,
            ))
        }
    }
}

pub(super) fn file_digest(path: &Path) -> Result<(String, File)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut file = options.open(path).context("open backend identity file")?;
    ensure!(
        file.metadata()?.is_file(),
        "backend identity must be a regular file"
    );
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    // 文件句柄保持至识别/缓存写入完成；Windows 拒绝期间的写入和删除。
    Ok((format!("{:x}", hash.finalize()), file))
}

#[cfg(test)]
#[path = "cached_tests.rs"]
mod tests;
