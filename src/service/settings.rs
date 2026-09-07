//! Allowed paths enter through authenticated Configure, never task submissions.
use crate::runtime::RuntimeContext;
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
    pub enterprise_snapshot: Option<PathBuf>,
    pub enterprise_data_dir: Option<PathBuf>,
    pub enterprise_discovery_root: Option<PathBuf>,
    pub enterprise_input: Option<PathBuf>,
    pub enterprise_key_file: Option<PathBuf>,
    pub enterprise_keys_file: Option<PathBuf>,
    pub enterprise_self_id: Option<i64>,
    pub enterprise_pid: Vec<u32>,
    pub image_cache_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enterprise_snapshot: Option<PathBuf>,
    pub enterprise_data_dir: Option<PathBuf>,
    pub enterprise_discovery_root: Option<PathBuf>,
    pub enterprise_input: Option<PathBuf>,
    pub enterprise_key_file: Option<PathBuf>,
    pub enterprise_keys_file: Option<PathBuf>,
    pub enterprise_self_id: Option<i64>,
    pub enterprise_pids: Vec<u32>,
    pub image_cache_dir: Option<PathBuf>,
    pub transcription_backend: String,
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
fn check_pids(pids: &[u32]) -> Result<()> {
    ensure!(
        pids.len() <= 16 && pids.iter().all(|p| *p > 0),
        "Invalid enterprise PIDs"
    );
    ensure!(
        pids.iter().collect::<std::collections::HashSet<_>>().len() == pids.len(),
        "Duplicate enterprise PID"
    );
    Ok(())
}
impl SettingsInput {
    pub fn validate_serialized(&self) -> Result<()> {
        check_pids(&self.enterprise_pid)?;
        for path in [
            &self.enterprise_snapshot,
            &self.enterprise_data_dir,
            &self.enterprise_discovery_root,
            &self.enterprise_input,
            &self.enterprise_key_file,
            &self.enterprise_keys_file,
            &self.image_cache_dir,
        ]
        .into_iter()
        .flatten()
        {
            check_path(path)?;
        }
        ensure!(
            self.enterprise_input.is_none()
                || (self.enterprise_data_dir.is_none() && self.enterprise_keys_file.is_none()),
            "Conflicting enterprise input modes"
        );
        Ok(())
    }
}
impl Settings {
    /// Recheck private serialized settings before use. This does not authorize new paths.
    pub fn validate_serialized(&self) -> Result<()> {
        check_pids(&self.enterprise_pids)?;
        ensure!(
            [
                "",
                "unconfigured",
                "unsupported",
                "local",
                "openai",
                "whisper_cpp"
            ]
            .contains(&self.transcription_backend.as_str()),
            "Invalid transcription backend"
        );
        for path in [
            &self.enterprise_snapshot,
            &self.enterprise_data_dir,
            &self.enterprise_discovery_root,
            &self.enterprise_input,
            &self.enterprise_key_file,
            &self.enterprise_keys_file,
            &self.image_cache_dir,
        ]
        .into_iter()
        .flatten()
        {
            check_path(path)?;
            ensure!(path.is_absolute(), "Serialized path must be absolute");
            ensure!(
                fs::canonicalize(path)? == *path,
                "Serialized path is not canonical"
            );
        }
        ensure!(
            !(self.enterprise_input.is_some() && self.enterprise_data_dir.is_some()),
            "企业微信单库输入和 Data 目录不能混用"
        );
        ensure!(
            !(self.enterprise_input.is_some() && self.enterprise_keys_file.is_some()),
            "企业微信单库输入不能使用逐库密钥清单"
        );
        ensure!(
            self.enterprise_input.is_none() || self.enterprise_key_file.is_some(),
            "企业微信单库输入需要 key-file"
        );
        for directory in [
            &self.enterprise_snapshot,
            &self.enterprise_data_dir,
            &self.enterprise_discovery_root,
            &self.image_cache_dir,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(directory.is_dir(), "Web 目录配置指向的不是目录");
        }
        for file in [
            &self.enterprise_input,
            &self.enterprise_key_file,
            &self.enterprise_keys_file,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(file.is_file(), "Web 文件配置指向的不是文件");
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
    let enterprise_self_id = match args.enterprise_self_id {
        Some(id) => Some(id),
        None => match raw.get("enterprise_self_id") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_i64().context("enterprise_self_id 必须为整数")?),
        },
    };
    let enterprise_pids = if args.enterprise_pid.is_empty() {
        match raw.get("enterprise_pids") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value::<Vec<u32>>(value.clone())
                .map_err(|_| anyhow::anyhow!("enterprise_pids 必须是 PID 数组"))?,
        }
    } else {
        args.enterprise_pid.clone()
    };
    ensure!(
        enterprise_pids.len() <= 16 && enterprise_pids.iter().all(|p| *p > 0),
        "企业微信 PID 范围无效"
    );
    let settings = Settings {
        enterprise_snapshot: path(&args.enterprise_snapshot, "enterprise_snapshot")?,
        enterprise_data_dir: path(&args.enterprise_data_dir, "enterprise_data_dir")?,
        enterprise_discovery_root: path(
            &args.enterprise_discovery_root,
            "enterprise_discovery_root",
        )?,
        enterprise_input: path(&args.enterprise_input, "enterprise_input")?,
        enterprise_key_file: path(&args.enterprise_key_file, "enterprise_key_file")?,
        enterprise_keys_file: path(&args.enterprise_keys_file, "enterprise_keys_file")?,
        enterprise_self_id,
        enterprise_pids,
        image_cache_dir: path(&args.image_cache_dir, "image_cache_dir")?,
        transcription_backend: match raw.get("transcription_backend") {
            None => "unconfigured".into(),
            Some(Value::String(s)) if ["local", "openai", "whisper_cpp"].contains(&s.as_str()) => {
                s.clone()
            }
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
    fn rejects_serialized_injection_and_invalid_paths() -> Result<()> {
        for value in [
            json!({"argv":["--config"]}),
            json!({"port":0}),
            json!({"open":false}),
        ] {
            assert!(serde_json::from_value::<SettingsInput>(value).is_err());
        }
        for pids in [vec![0], vec![1, 1], vec![1; 17]] {
            assert!(SettingsInput {
                enterprise_pid: pids,
                ..Default::default()
            }
            .validate_serialized()
            .is_err());
        }
        for path in ["", "bad\u{0000}path", "bad\npath"] {
            assert!(SettingsInput {
                image_cache_dir: Some(path.into()),
                ..Default::default()
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
            enterprise_key_file: Some(fs::canonicalize(root.path())?),
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
    fn configured_batch_key_combination_preserves_single_database_boundary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("personal-keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir)?;
        fs::create_dir(root.path().join("enterprise"))?;
        fs::write(root.path().join("global.key"), b"SYNTHETIC")?;
        fs::write(root.path().join("per-database.json"), b"{}")?;
        fs::write(root.path().join("single.db"), b"SYNTHETIC")?;
        let mut raw = serde_json::to_value(&config)?;
        raw["enterprise_data_dir"] = json!("enterprise");
        raw["enterprise_key_file"] = json!("global.key");
        raw["enterprise_keys_file"] = json!("per-database.json");
        fs::write(&config_path, serde_json::to_vec(&raw)?)?;
        let runtime =
            RuntimeContext::from_config(config_path.clone(), config, root.path().join("runtime"))?;
        let settings = load(&runtime, &SettingsInput::default())?;
        assert_eq!(
            settings.enterprise_key_file,
            Some(fs::canonicalize(root.path().join("global.key"))?)
        );
        assert_eq!(
            settings.enterprise_keys_file,
            Some(fs::canonicalize(root.path().join("per-database.json"))?)
        );

        raw["enterprise_data_dir"] = Value::Null;
        raw["enterprise_input"] = json!("single.db");
        fs::write(&config_path, serde_json::to_vec(&raw)?)?;
        let error = load(&runtime, &SettingsInput::default())
            .err()
            .context("单库混用应被拒绝")?;
        assert!(error.to_string().contains("单库输入不能使用逐库密钥清单"));
        raw["enterprise_keys_file"] = Value::Null;
        fs::write(&config_path, serde_json::to_vec(&raw)?)?;
        assert!(load(&runtime, &SettingsInput::default())?
            .enterprise_input
            .is_some());
        assert!(!runtime.root.exists());
        assert!(!runtime.config.decrypted_dir.exists());
        Ok(())
    }
}
