//! 显式后端参数及单文件、聊天转录入口；不自动读取账号和云端凭证。
use crate::toolkit::asr::backend::BackendId;
#[cfg(test)]
use crate::toolkit::asr::backend::Entry;
use crate::toolkit::asr::{self, local, openai, Backend, OfflineMedia};
use anyhow::{ensure, Context, Result};
use std::{fs::File, io::Read, time::Duration};

#[cfg(test)]
use crate::service::operation_requests::asr::BackendKind;

pub use crate::service::operation_requests::asr::BackendArgs;

impl BackendArgs {
    pub fn build(self) -> Result<Backend> {
        let backend = self.validate_explicit()?;
        match backend {
            BackendId::PythonWhisper => unreachable!("explicit validation rejected Python"),
            BackendId::WhisperCpp => {
                let mut config = local::LocalConfig::new(
                    self.whisper_binary
                        .context("--whisper-binary is required")?,
                    self.whisper_model.context("--whisper-model is required")?,
                );
                config.language = self.language;
                if let Some(threads) = self.threads {
                    config.threads = threads;
                }
                config.timeout = Duration::from_secs(self.timeout_seconds);
                config.output_format = local::OutputFormat::Json;
                config.temp_root = self.temp_root;
                Ok(Backend::Local(config))
            }
            BackendId::OpenAiCompatible => {
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

#[cfg(test)]
mod backend_selection_tests {
    use super::*;

    #[test]
    fn wire_aliases_preserve_native_identity() {
        for (names, expected) in [
            (vec!["local", "whisper_cpp"], BackendId::WhisperCpp),
            (
                vec!["explicit-open-ai", "openai", "openai_compatible"],
                BackendId::OpenAiCompatible,
            ),
            (vec!["python_whisper"], BackendId::PythonWhisper),
        ] {
            for name in names {
                let kind: BackendKind = serde_json::from_value(serde_json::json!(name)).unwrap();
                assert_eq!(kind.identity(Entry::Native), expected);
            }
        }
        let old: BackendKind = serde_json::from_str("\"ExplicitOpenAi\"").unwrap();
        assert_eq!(old.identity(Entry::Native), BackendId::OpenAiCompatible);
    }

    #[test]
    fn explicit_preflight_rejects_upload_and_engine_mix_before_io() {
        for kind in [BackendKind::Local, BackendKind::WhisperCpp] {
            let mut args = BackendArgs {
                backend: kind,
                whisper_binary: Some("nonexistent-binary".into()),
                whisper_model: Some("nonexistent-model".into()),
                ..Default::default()
            };
            assert_eq!(args.validate_explicit().unwrap(), BackendId::WhisperCpp);
            args.allow_upload = true;
            assert!(args.validate_explicit().is_err());
        }
        let mut cloud = BackendArgs {
            backend: BackendKind::ExplicitOpenAi,
            openai_base_url: Some("https://example.invalid/v1".into()),
            openai_model: Some("test".into()),
            api_key_file: Some("not-read".into()),
            ..Default::default()
        };
        assert!(cloud.validate_explicit().is_err());
        cloud.allow_upload = true;
        assert!(cloud.validate_explicit().is_ok());
        cloud.temp_root = Some("not-created".into());
        assert!(cloud.validate_explicit().is_err());
        assert!(BackendArgs {
            backend: BackendKind::PythonWhisper,
            ..Default::default()
        }
        .validate_explicit()
        .is_err());
    }
}

pub use crate::service::operation_requests::asr::TranscribeAudioNativeArgs;

pub use crate::service::operation_requests::asr::TranscribeChatNativeArgs;

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
    finish_chat_report(&report)
}

fn finish_chat_report(report: &asr::writeback::WritebackReport) -> Result<()> {
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
    crate::ipc::outcome::BusinessOutcome::from_counts(
        report.transcribed.saturating_add(report.skipped_existing) as u64,
        report.failed as u64,
    )
    .require_success()?;
    Ok(())
}

#[test]
fn chat_report_preserves_partial_classification() {
    use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
    for (transcribed, skipped_existing, failed, expected) in [
        (1, 0, 0, BusinessOutcome::Success),
        (1, 0, 1, BusinessOutcome::Partial),
        (0, 1, 1, BusinessOutcome::Partial),
        (0, 0, 1, BusinessOutcome::Failure),
    ] {
        let report = asr::writeback::WritebackReport {
            transcribed,
            skipped_existing,
            failed,
            skipped_non_voice: 3,
            ..Default::default()
        };
        let actual = finish_chat_report(&report).map_or_else(
            |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
            |_| BusinessOutcome::Success,
        );
        assert_eq!(actual, expected);
    }
}
