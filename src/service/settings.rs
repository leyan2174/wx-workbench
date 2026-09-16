//! Allowed paths enter through authenticated Configure, never task submissions.
use crate::runtime::RuntimeContext;
use crate::service::operation_requests::asr::BackendId;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsInput {
    pub image_cache_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub image_cache_dir: Option<PathBuf>,
    pub transcription_backend: String,
}

pub struct TranscriptionCapabilities {
    pub available: bool,
    pub python_whisper: bool,
    pub requires_upload: bool,
}

const MAX_PATH_BYTES: usize = 32768;
fn check_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("Path must be Unicode")?;
    ensure!(
        !text.is_empty() && text.len() <= MAX_PATH_BYTES,
        "Invalid path length"
    );
    ensure!(
        !text.chars().any(char::is_control),
        "Control characters in path"
    );
    Ok(())
}
impl SettingsInput {
    pub fn validate_serialized(&self) -> Result<()> {
        if let Some(path) = &self.image_cache_dir {
            check_path(path)?;
        }
        Ok(())
    }
}
impl Settings {
    pub fn transcription_capabilities(&self) -> TranscriptionCapabilities {
        let backend = BackendId::parse(&self.transcription_backend).ok();
        TranscriptionCapabilities {
            available: backend.is_some(),
            python_whisper: backend == Some(BackendId::PythonWhisper),
            requires_upload: backend == Some(BackendId::OpenAiCompatible),
        }
    }

    /// Recheck private serialized settings before use. This does not authorize new paths.
    pub fn validate_serialized(&self) -> Result<()> {
        ensure!(
            ["", "unconfigured", "unsupported"].contains(&self.transcription_backend.as_str())
                || BackendId::parse(&self.transcription_backend).is_ok(),
            "Invalid transcription backend"
        );
        if let Some(path) = &self.image_cache_dir {
            check_path(path)?;
            ensure!(path.is_absolute(), "Serialized path must be absolute");
            ensure!(
                fs::canonicalize(path)? == *path,
                "Serialized path is not canonical"
            );
        }
        if let Some(directory) = &self.image_cache_dir {
            ensure!(directory.is_dir(), "Web 目录配置指向的不是目录");
        }

        Ok(())
    }
}
pub fn load(runtime: &RuntimeContext, args: &SettingsInput) -> Result<Settings> {
    args.validate_serialized()?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    fs::File::open(&runtime.config_path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "配置超过读取限额");
    let raw: Value = serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("配置格式无效"))?;
    ensure!(raw.is_object(), "Configuration must be an object");
    let base = runtime.config_path.parent().context("配置缺少父目录")?;
    let path = |explicit: &Option<PathBuf>, field: &str| -> Result<Option<PathBuf>> {
        let selected = match explicit {
            Some(path) => {
                check_path(path)?;
                Some(std::path::absolute(path)?)
            }
            None => match raw.get(field) {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s.is_empty() => None,
                Some(Value::String(s)) => Some(if Path::new(s).is_absolute() {
                    PathBuf::from(s)
                } else {
                    base.join(s)
                }),
                _ => anyhow::bail!("Web 路径配置必须是字符串"),
            },
        };
        if let Some(path) = &selected {
            check_path(path)?;
        }
        selected
            .map(fs::canonicalize)
            .transpose()
            .map_err(|_| anyhow::anyhow!("Web 配置目录或文件不可用"))
    };
    let settings = Settings {
        image_cache_dir: path(&args.image_cache_dir, "image_cache_dir")?,
        transcription_backend: match raw.get("transcription_backend") {
            None => "unconfigured".into(),
            Some(Value::String(s)) => BackendId::parse(s)
                .map(|id| id.as_str().to_owned())
                .unwrap_or_else(|_| "unsupported".into()),
            _ => "unsupported".into(),
        },
    };
    settings.validate_serialized()?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn asr_settings_accept_only_canonical_names() {
        for name in ["whisper_cpp", "python_whisper", "openai_compatible"] {
            assert!(Settings {
                transcription_backend: name.into(),
                ..Default::default()
            }
            .validate_serialized()
            .is_ok());
        }
        assert!(Settings {
            transcription_backend: "unknown-engine".into(),
            ..Default::default()
        }
        .validate_serialized()
        .is_err());
        for name in ["local", "openai", "explicit-open-ai", "ExplicitOpenAi"] {
            assert!(Settings {
                transcription_backend: name.into(),
                ..Default::default()
            }
            .validate_serialized()
            .is_err());
        }
    }

    #[test]
    fn transcription_capabilities_report_canonical_engines() {
        for (name, available, python, upload) in [
            ("python_whisper", true, true, false),
            ("whisper_cpp", true, false, false),
            ("openai_compatible", true, false, true),
            ("local", false, false, false),
            ("openai", false, false, false),
            ("unconfigured", false, false, false),
        ] {
            let capabilities = Settings {
                transcription_backend: name.into(),
                ..Default::default()
            }
            .transcription_capabilities();
            assert_eq!(
                (
                    capabilities.available,
                    capabilities.python_whisper,
                    capabilities.requires_upload
                ),
                (available, python, upload),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_serialized_injection_and_invalid_paths() -> Result<()> {
        for value in [
            json!({"argv":["--config"]}),
            json!({"port":0}),
            json!({"open":false}),
        ] {
            assert!(serde_json::from_value::<SettingsInput>(value).is_err());
        }
        for path in ["", "bad\u{0000}path", "bad\npath"] {
            assert!(SettingsInput {
                image_cache_dir: Some(path.into()),
            }
            .validate_serialized()
            .is_err());
        }
        assert!(Settings {
            image_cache_dir: Some("relative".into()),
            ..Default::default()
        }
        .validate_serialized()
        .is_err());
        let root = tempfile::tempdir()?;
        let file = root.path().join("file");
        fs::write(&file, b"synthetic")?;
        assert!(Settings {
            image_cache_dir: Some(fs::canonicalize(&file)?),
            ..Default::default()
        }
        .validate_serialized()
        .is_err());
        assert!(Settings {
            image_cache_dir: Some(fs::canonicalize(root.path())?),
            ..Default::default()
        }
        .validate_serialized()
        .is_ok());
        Ok(())
    }

    #[test]
    fn configured_image_cache_is_canonical_without_account_writes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
            key_store: None,
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("personal-keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir)?;
        fs::create_dir(root.path().join("images"))?;
        let mut raw = serde_json::to_value(&config)?;
        raw["image_cache_dir"] = json!("images");
        // Obsolete config must not open or validate paths for removed features.
        raw["enterprise_snapshot"] = json!("missing-obsolete-snapshot");
        fs::write(&config_path, serde_json::to_vec(&raw)?)?;
        let runtime =
            RuntimeContext::from_config(config_path.clone(), config, root.path().join("runtime"))?;
        let settings = load(&runtime, &SettingsInput::default())?;
        assert_eq!(
            settings.image_cache_dir,
            Some(fs::canonicalize(root.path().join("images"))?)
        );

        assert!(!runtime.root.exists());
        assert!(!runtime.config.decrypted_dir.exists());
        Ok(())
    }
}
