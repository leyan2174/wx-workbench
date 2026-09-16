//! Backend identities and pure configuration validation; never grants host permissions.
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendId {
    WhisperCpp,
    PythonWhisper,
    OpenAiCompatible,
}

impl BackendId {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "whisper_cpp" => Ok(Self::WhisperCpp),
            "python_whisper" => Ok(Self::PythonWhisper),
            "openai_compatible" => Ok(Self::OpenAiCompatible),
            _ => bail!("unsupported ASR backend; no fallback performed"),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WhisperCpp => "whisper_cpp",
            Self::PythonWhisper => "python_whisper",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    pub fn configured(config: &Value) -> Result<Self> {
        ensure!(
            config.is_object(),
            "transcription configuration must be an object"
        );
        let name = config
            .get("transcription_backend")
            .context("transcription_backend is required")?
            .as_str()
            .context("transcription_backend must be a string")?;
        Self::parse(name)
    }
}

/// Presence only: values, credentials, paths and host grants remain entry-owned.
#[derive(Default)]
pub struct Options {
    pub cloud: bool,
    pub cpp_paths: bool,
    pub threads: Option<usize>,
    pub temp_root: bool,
    pub allow_upload: bool,
}

impl Options {
    pub fn validate(&self, backend: BackendId, language: &str, timeout: u64) -> Result<()> {
        ensure!(timeout > 0, "ASR timeout must be positive");
        ensure!(!language.trim().is_empty(), "language must not be empty");
        ensure!(self.threads != Some(0), "threads must be positive");
        match backend {
            BackendId::OpenAiCompatible => {
                ensure!(
                    self.allow_upload,
                    "audio upload requires explicit authorization (--allow-upload)"
                );
                ensure!(
                    !self.cpp_paths && self.threads.is_none() && !self.temp_root,
                    "local options cannot be used with openai_compatible"
                );
            }
            BackendId::WhisperCpp | BackendId::PythonWhisper => {
                ensure!(
                    !self.cloud && !self.allow_upload,
                    "cloud options require openai_compatible"
                );
                if backend == BackendId::PythonWhisper {
                    ensure!(
                        !self.cpp_paths,
                        "whisper.cpp paths cannot be used with python_whisper"
                    );
                }
            }
        }
        Ok(())
    }
}

pub fn python_model(config: &Value) -> Result<String> {
    match config.get("local_whisper_model") {
        None => Ok("base".into()),
        Some(value) => Ok(value
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .context("invalid configured local model")?
            .to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_canonical_names_are_accepted() {
        for id in [
            BackendId::WhisperCpp,
            BackendId::PythonWhisper,
            BackendId::OpenAiCompatible,
        ] {
            assert_eq!(BackendId::parse(id.as_str()).unwrap(), id);
        }
        for name in [
            "local",
            "Local",
            "openai",
            "explicit-open-ai",
            "ExplicitOpenAi",
        ] {
            assert!(BackendId::parse(name).is_err());
        }
        for value in [
            json!({}),
            json!({"transcription_backend":null}),
            json!({"transcription_backend":"unknown"}),
            json!([]),
        ] {
            assert!(BackendId::configured(&value).is_err());
        }
    }

    #[test]
    fn authorization_and_mixed_options_fail_without_io() {
        let cloud = BackendId::OpenAiCompatible;
        assert!(Options::default().validate(cloud, "auto", 1).is_err());
        let allowed = Options {
            allow_upload: true,
            ..Options::default()
        };
        assert!(allowed.validate(cloud, "auto", 1).is_ok());
        for id in [BackendId::WhisperCpp, BackendId::PythonWhisper] {
            assert!(allowed.validate(id, "auto", 1).is_err());
            assert!(Options {
                cloud: true,
                ..Options::default()
            }
            .validate(id, "auto", 1)
            .is_err());
        }
        for options in [
            Options {
                cpp_paths: true,
                ..Options::default()
            },
            Options {
                threads: Some(1),
                ..Options::default()
            },
            Options {
                temp_root: true,
                ..Options::default()
            },
        ] {
            assert!(Options {
                allow_upload: true,
                ..options
            }
            .validate(cloud, "auto", 1)
            .is_err());
        }
        assert!(Options {
            cpp_paths: true,
            ..Options::default()
        }
        .validate(BackendId::PythonWhisper, "auto", 1)
        .is_err());
        assert!(Options::default()
            .validate(BackendId::WhisperCpp, " ", 1)
            .is_err());
        assert!(Options::default()
            .validate(BackendId::WhisperCpp, "auto", 0)
            .is_err());
        assert!(python_model(&json!({"local_whisper_model":" "})).is_err());
    }
}
