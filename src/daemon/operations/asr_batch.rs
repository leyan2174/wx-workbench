//! 旧 transcribe-chat 的自动数据库入口；待 main 接线及集中验证。
use super::asr::BackendArgs;
use crate::toolkit::asr::backend::{self, BackendId, Entry};
pub use crate::toolkit::asr::batch::{BatchTranscriber, Report};
use crate::{
    runtime::RuntimeContext,
    toolkit::asr::{batch::CacheOptions, local, local_python, openai, Backend},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub use crate::service::operation_requests::asr_batch::Args;

pub use crate::service::operation_requests::asr_batch::BatchArgs;

pub fn cmd(args: Args) -> Result<()> {
    let runtime = RuntimeContext::load()?;
    let report = cmd_for(&runtime, args)?;
    finish_report(&report)
}

fn finish_report(report: &Report) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&report)?);
    crate::ipc::outcome::BusinessOutcome::from_counts(
        report.transcribed.saturating_add(report.skipped_existing) as u64,
        report.failed.saturating_add(report.warnings.len()) as u64,
    )
    .require_success()?;
    Ok(())
}

#[test]
fn batch_report_counts_persistence_warnings_but_not_engine_warnings() {
    use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
    for (transcribed, skipped_existing, failed, warning, expected) in [
        (1, 0, 0, false, BusinessOutcome::Success),
        (1, 0, 1, false, BusinessOutcome::Partial),
        (0, 1, 1, false, BusinessOutcome::Partial),
        (0, 0, 1, false, BusinessOutcome::Failure),
        (1, 0, 0, true, BusinessOutcome::Partial),
        (0, 0, 0, true, BusinessOutcome::Failure),
    ] {
        let mut report = Report {
            transcribed,
            skipped_existing,
            failed,
            skipped_non_voice: 3,
            engine_warnings: vec!["informational backend identity".into()],
            ..Default::default()
        };
        if warning {
            report.warnings.push(crate::toolkit::asr::batch::Warning {
                username: "synthetic".into(),
                source: "message_0.db".into(),
                local_id: 1,
                error: "synthetic persistence failure".into(),
            });
        }
        let actual = finish_report(&report).map_or_else(
            |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
            |_| BusinessOutcome::Success,
        );
        assert_eq!(actual, expected);
    }
}

/// main 可沿用已经固定的账号，不重新加载全局运行上下文。
pub fn cmd_for(runtime: &RuntimeContext, args: Args) -> Result<Report> {
    let output = args.output.unwrap_or_else(|| default_output(&args.input));
    let mut transcriber = prepare(runtime, args.batch)?;
    transcriber.process_file(&args.input, &output)
}

/// main 仅在实际请求转录时调用；计划及 dry-run 不触碰配置、音频和云端。
pub fn prepare(runtime: &RuntimeContext, args: BatchArgs) -> Result<BatchTranscriber> {
    let backend = if args.explicit_backend {
        args.backend.build()?
    } else {
        configured_backend(runtime, args.backend)?
    };
    let file_name = if args.asr_cache_name.is_empty() {
        CacheOptions::default().file_name
    } else {
        args.asr_cache_name
    };
    BatchTranscriber::new(runtime, backend, CacheOptions { file_name })
}

fn default_output(input: &Path) -> PathBuf {
    let mut name = input.file_stem().unwrap_or_default().to_os_string();
    name.push("_transcribed.json");
    input.with_file_name(name)
}

/// 固定配置显式选定引擎，不因运行失败切换后端；此函数不运行模型。
fn configured_backend(runtime: &RuntimeContext, overrides: BackendArgs) -> Result<Backend> {
    let config: Value = serde_json::from_slice(
        &fs::read(&runtime.config_path)
            .context("read fixed-account transcription configuration")?,
    )?;
    ensure!(
        config.is_object(),
        "transcription configuration must be an object"
    );
    let backend = match overrides.backend {
        crate::service::operation_requests::asr::BackendKind::Local => {
            BackendId::configured(&config)?
        }
        selected => selected.identity(Entry::ConfiguredBatch),
    };
    overrides.validate_for(backend)?;
    match backend {
        BackendId::PythonWhisper => {
            let model = backend::python_model(&config)?;
            let local = local_python::LocalPythonConfig::discover(
                model,
                (overrides.language != "auto").then_some(overrides.language),
                overrides.threads,
                Duration::from_secs(overrides.timeout_seconds),
                overrides
                    .temp_root
                    .unwrap_or_else(|| runtime.directory.clone()),
                runtime
                    .config_path
                    .parent()
                    .context("selected configuration parent missing")?
                    .to_owned(),
            )?;
            Ok(Backend::LegacyPythonLocal(local))
        }
        BackendId::WhisperCpp => {
            let base = runtime
                .config_path
                .parent()
                .context("config parent missing")?;
            let configured_path = |field: &str| -> Result<PathBuf> {
                let path = PathBuf::from(required_string(&config, field)?);
                Ok(if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                })
            };
            let binary = match overrides.whisper_binary {
                Some(path) => path,
                None => configured_path("whisper_cpp_binary")?,
            };
            let model = match overrides.whisper_model {
                Some(path) => path,
                None => match config.get("whisper_cpp_model") {
                    Some(value) => {
                        let model = value
                            .as_str()
                            .context("whisper_cpp_model must be a string")?;
                        if model.is_empty() {
                            discover_cpp_model()?
                        } else {
                            configured_path("whisper_cpp_model")?
                        }
                    }
                    None => discover_cpp_model()?,
                },
            };
            let mut local = local::LocalConfig::new(binary, model);
            local.output_format = local::OutputFormat::Json;
            local.language = match config.get("whisper_cpp_language") {
                Some(value) => value
                    .as_str()
                    .context("whisper_cpp_language must be a string")?
                    .to_owned(),
                None => "zh".into(),
            };
            ensure!(
                !local.language.is_empty(),
                "whisper_cpp_language must not be empty"
            );
            let threads = match config.get("whisper_cpp_threads") {
                Some(value) => value
                    .as_u64()
                    .context("whisper_cpp_threads must be a nonnegative integer")?,
                None => 0,
            };
            // 旧配置 0 表示自动线程；本地后端构造默认值提供其原生自动策略。
            if threads != 0 {
                local.threads = usize::try_from(threads)
                    .context("whisper_cpp_threads exceeds platform range")?;
            } else if overrides.threads.is_none() {
                eprintln!("[asr-batch] legacy whisper_cpp_threads=0 uses native automatic threads (capped at 8); use --threads for an explicit value");
            }
            if let Some(threads) = overrides.threads {
                ensure!(threads > 0, "threads must be positive");
                local.threads = threads;
            }
            if overrides.language != "auto" {
                local.language = overrides.language;
            }
            local.timeout = Duration::from_secs(overrides.timeout_seconds);
            local.temp_root = overrides.temp_root;
            Ok(Backend::Local(local))
        }
        BackendId::OpenAiCompatible => {
            if overrides.api_key_file.is_some() {
                return BackendArgs {
                    backend: crate::service::operation_requests::asr::BackendKind::ExplicitOpenAi,
                    openai_base_url: Some(
                        overrides
                            .openai_base_url
                            .clone()
                            .unwrap_or_else(|| "https://api.openai.com/v1".into()),
                    ),
                    openai_model: Some(
                        overrides
                            .openai_model
                            .clone()
                            .unwrap_or_else(|| "whisper-1".into()),
                    ),
                    ..overrides
                }
                .build();
            }
            Backend::explicit_openai(
                openai::OpenAiConfig {
                    base_url: overrides
                        .openai_base_url
                        .unwrap_or_else(|| "https://api.openai.com/v1".into()),
                    model: overrides.openai_model.unwrap_or_else(|| "whisper-1".into()),
                    language: (overrides.language != "auto").then_some(overrides.language),
                    api_key: configured_api_key(&config)?,
                    timeout: Duration::from_secs(overrides.timeout_seconds),
                    max_audio_bytes: openai::OPENAI_AUDIO_LIMIT_BYTES,
                },
                true,
            )
        }
    }
}

fn configured_api_key(config: &Value) -> Result<String> {
    configured_api_key_with(config, |name| std::env::var(name))
}

fn configured_api_key_with(
    config: &Value,
    read_env: impl FnOnce(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<String> {
    // 调用方已检查上传授权和 CLI 凭据；显式环境来源失败时不得回退到旧明文 key。
    let Some(name) = config.get("openai_api_key_env") else {
        return Ok(required_string(config, "openai_api_key")?.to_owned());
    };
    let name = name
        .as_str()
        .context("openai_api_key_env must be a string")?;
    crate::toolkit::setup::valid_env_name(name)?;
    // VarError::NotUnicode 可能携带凭据原文，不能作为错误上下文输出。
    let key = match read_env(name) {
        Ok(value) => zeroize::Zeroizing::new(value),
        Err(std::env::VarError::NotPresent) => {
            bail!("configured OpenAI credential environment variable is not set")
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("configured OpenAI credential environment variable is not valid Unicode")
        }
    };
    ensure!(
        key.len() <= 16_384,
        "configured OpenAI credential exceeds limit"
    );
    ensure!(
        !key.trim().is_empty(),
        "configured OpenAI credential environment variable is empty"
    );
    Ok(key.trim().to_owned())
}

fn required_string<'a>(config: &'a Value, field: &str) -> Result<&'a str> {
    config
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .with_context(|| format!("explicit nonempty {field} is required"))
}

fn discover_cpp_model() -> Result<PathBuf> {
    // 对齐旧 Windows expanduser 及三个搜索目录的优先级，只在未指定模型时发现。
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| {
            std::env::var_os("HOMEPATH").map(|path| {
                let mut home = std::env::var_os("HOMEDRIVE").unwrap_or_default();
                home.push(path);
                home
            })
        })
        .context(
            "legacy model discovery requires USERPROFILE or HOMEPATH; specify --whisper-model",
        )?;
    let home = std::path::absolute(PathBuf::from(home))?;
    for name in ["whisper-models", "models", "Downloads"] {
        let directory = home.join(name);
        if !directory.is_dir() {
            continue;
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&directory).context("read legacy model search directory")? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("ggml-") && name.ends_with(".bin") {
                candidates.push((name, entry.path()));
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((name, path)) = candidates.into_iter().next() {
            ensure!(
                path.is_file(),
                "first legacy model candidate is not a regular file"
            );
            eprintln!("[asr-batch] legacy model discovery selected {name}; use --whisper-model to pin explicitly");
            return path
                .canonicalize()
                .context("resolve discovered whisper.cpp model");
        }
    }
    bail!("no ggml-*.bin model found in legacy search directories; configure whisper_cpp_model or --whisper-model")
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "SYNTHETIC_ASR_SECRET_MUST_NOT_LEAK";

    fn assert_redacted(error: anyhow::Error) -> String {
        let rendered = format!("{error} {error:#} {error:?}");
        assert!(!rendered.contains(SECRET));
        rendered
    }

    #[test]
    fn configured_env_wins_over_inline_and_reads_only_named_variable() {
        let config = json!({"openai_api_key_env":"_ASR_KEY_9", "openai_api_key":"inline"});
        let key = configured_api_key_with(&config, |name| {
            assert_eq!(name, "_ASR_KEY_9");
            Ok(format!("  {SECRET}\n"))
        })
        .unwrap();
        assert_eq!(key, SECRET);
    }

    #[test]
    fn absent_env_field_uses_inline_without_any_environment_lookup() {
        let config = json!({"openai_api_key":SECRET});
        assert_eq!(
            configured_api_key_with(&config, |_| panic!("unexpected environment lookup")).unwrap(),
            SECRET
        );
        assert_redacted(
            configured_api_key_with(&json!({}), |_| panic!("unexpected environment lookup"))
                .unwrap_err(),
        );
    }

    #[test]
    fn invalid_env_field_never_reads_environment_or_falls_back() {
        for name in [
            json!(null),
            json!(17),
            json!(true),
            json!([]),
            json!({}),
            json!(""),
            json!("9ASR"),
            json!("ASR-KEY"),
            json!("ASR=KEY"),
            json!(" ASR"),
            json!("ASR\n"),
            json!("ASR\0KEY"),
            json!("密钥"),
            json!("A".repeat(129)),
            json!(format!("{SECRET}-invalid")),
        ] {
            let config = json!({"openai_api_key_env":name, "openai_api_key":SECRET});
            assert_redacted(
                configured_api_key_with(&config, |_| panic!("invalid name was read")).unwrap_err(),
            );
        }
    }

    #[test]
    fn env_name_length_boundary_is_accepted() {
        let name = "A".repeat(128);
        let config = json!({"openai_api_key_env":name});
        assert_eq!(
            configured_api_key_with(&config, |selected| {
                assert_eq!(selected, name);
                Ok(SECRET.into())
            })
            .unwrap(),
            SECRET
        );
    }

    #[test]
    fn missing_or_non_unicode_env_does_not_fallback_or_leak() {
        let config = json!({"openai_api_key_env":"ASR_KEY", "openai_api_key":SECRET});
        for error in [
            std::env::VarError::NotPresent,
            std::env::VarError::NotUnicode(std::ffi::OsString::from(SECRET)),
        ] {
            assert_redacted(configured_api_key_with(&config, |_| Err(error)).unwrap_err());
        }
    }

    #[test]
    fn empty_or_oversized_env_does_not_fallback_or_leak() {
        let config = json!({"openai_api_key_env":"ASR_KEY", "openai_api_key":SECRET});
        for value in [String::new(), " \t\r\n".into(), SECRET.repeat(600)] {
            assert_redacted(configured_api_key_with(&config, |_| Ok(value)).unwrap_err());
        }
        let boundary = "x".repeat(16_384);
        assert_eq!(
            configured_api_key_with(&config, |_| Ok(boundary.clone())).unwrap(),
            boundary
        );
    }

    fn cloud_runtime(root: &Path) -> RuntimeContext {
        let config_path = root.join("config.json");
        fs::write(
            &config_path,
            serde_json::to_vec(&json!({
                "transcription_backend":"openai", "openai_api_key_env":null, "openai_api_key":SECRET
            }))
            .unwrap(),
        )
        .unwrap();
        RuntimeContext {
            config: crate::config::Config {
                key_store: None,
                db_dir: root.join("unused-db"),
                keys_file: root.join("unused-keys"),
                decrypted_dir: root.join("unused-decrypted"),
                wechat_process: String::new(),
            },
            config_path,
            root: root.to_owned(),
            id: "synthetic".into(),
            directory: root.join("unused-runtime"),
        }
    }

    #[test]
    fn configured_canonical_selection_and_alias_authorization() {
        let root = tempfile::tempdir().unwrap();
        let runtime = cloud_runtime(root.path());
        for name in ["local", "python_whisper"] {
            fs::write(
                &runtime.config_path,
                serde_json::to_vec(&json!({"transcription_backend":name})).unwrap(),
            )
            .unwrap();
            let args = BackendArgs {
                whisper_binary: Some("not-opened".into()),
                ..Default::default()
            };
            let error = configured_backend(&runtime, args).unwrap_err();
            assert!(error.to_string().contains("python_whisper"));
        }
        for name in ["openai", "openai_compatible"] {
            fs::write(
                &runtime.config_path,
                serde_json::to_vec(&json!({"transcription_backend":name,
                "openai_api_key_env":null}))
                .unwrap(),
            )
            .unwrap();
            let error = configured_backend(&runtime, BackendArgs::default()).unwrap_err();
            assert!(error.to_string().contains("authorization"));
        }
        fs::write(
            &runtime.config_path,
            serde_json::to_vec(&json!({
                "transcription_backend":"whisper_cpp", "whisper_cpp_binary":"not-opened",
                "whisper_cpp_model":"not-opened.bin"
            }))
            .unwrap(),
        )
        .unwrap();
        for kind in [
            crate::service::operation_requests::asr::BackendKind::Local,
            crate::service::operation_requests::asr::BackendKind::WhisperCpp,
        ] {
            let selected = configured_backend(
                &runtime,
                BackendArgs {
                    backend: kind,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(selected.identity(), BackendId::WhisperCpp);
        }
    }

    #[test]
    fn cli_key_file_precedes_invalid_configured_env() {
        let root = tempfile::tempdir().unwrap();
        let runtime = cloud_runtime(root.path());
        let key_file = root.path().join("synthetic-key.txt");
        fs::write(&key_file, SECRET).unwrap();
        let backend = configured_backend(
            &runtime,
            BackendArgs {
                allow_upload: true,
                api_key_file: Some(key_file),
                openai_base_url: Some("http://127.0.0.1:9/v1".into()),
                ..BackendArgs::default()
            },
        )
        .unwrap();
        assert!(matches!(backend, Backend::ExplicitOpenAi { .. }));
        assert!(!format!("{backend:?}").contains(SECRET));
        assert!(!runtime.directory.exists());
    }

    #[test]
    fn upload_denial_precedes_cli_file_and_configured_env_access() {
        let root = tempfile::tempdir().unwrap();
        let runtime = cloud_runtime(root.path());
        let error = configured_backend(
            &runtime,
            BackendArgs {
                api_key_file: Some(root.path().join("missing-key.txt")),
                ..BackendArgs::default()
            },
        )
        .unwrap_err();
        assert!(assert_redacted(error).contains("--allow-upload"));
        assert!(!runtime.directory.exists());
    }
}
