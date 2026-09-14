use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Backend {
    Local,
    WhisperCpp,
    Openai,
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    pub check: bool,
    /// 显式配置文件；缺省复用 WX_CLI_CONFIG 和现有配置定位逻辑
    pub config_path: Option<PathBuf>,
    /// 明确选择 db_storage 或含 db_storage 的账号目录
    pub db_dir: Option<PathBuf>,
    pub backend: Option<Backend>,
    pub whisper_binary: Option<PathBuf>,
    pub whisper_model: Option<PathBuf>,
    pub local_model: Option<String>,
    /// 仅保存凭据环境变量名；能力检查只报告是否已设置，不保存或回显其值
    pub openai_key_env: Option<String>,
    pub interactive: bool,
    pub dry_run: bool,
    pub apply: bool,
    /// 明确确认写入；非交互 apply 必须同时提供
    pub yes: bool,
}

pub(crate) fn argument_fingerprint(args: &Args, path: &Path) -> Result<String> {
    let mut normalized = args.clone();
    normalized.config_path = Some(path.to_owned());
    normalized.apply = false;
    normalized.yes = false;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&normalized)?)
    ))
}
