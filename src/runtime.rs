//! 账号运行身份：统一命名管道、缓存、日志与进程记录的隔离边界。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::config::{self, Config};

#[derive(Debug, Clone)]
pub struct RuntimeContext {
    pub config: Config,
    pub config_path: PathBuf,
    pub root: PathBuf,
    pub id: String,
    pub directory: PathBuf,
}

impl RuntimeContext {
    /// Missing or malformed account configuration still permits service-side setup/offline work.
    pub(crate) fn for_operation() -> Result<Self> {
        Self::load().or_else(|_| Self::bootstrap())
    }

    pub(crate) fn bootstrap() -> Result<Self> {
        let config_path = normalized_path(&config::find_config_file()?)?;
        let root = normalized_path(&config::cli_dir())?;
        let mut digest = Sha256::new();
        digest.update(b"wx-cli-bootstrap-v1\0");
        for path in [&config_path, &root] {
            digest.update(path.to_string_lossy().to_lowercase().as_bytes());
            digest.update([0]);
        }
        let id = format!("{:x}", digest.finalize());
        let directory = root.join("bootstrap").join(&id);
        Ok(Self {
            config: Config {
                db_dir: directory.join("no-account"),
                keys_file: directory.join("no-keys"),
                decrypted_dir: directory.join("no-cache"),
                wechat_process: String::new(),
            },
            config_path,
            root,
            id,
            directory,
        })
    }

    pub(crate) fn is_bootstrap(&self) -> bool {
        self.directory == self.root.join("bootstrap").join(&self.id)
    }

    pub fn load() -> Result<Self> {
        let config_path = config::find_config_file()?;
        let config = config::load_config_at(&config_path)?;
        Self::from_config(config_path, config, config::cli_dir())
    }

    pub(crate) fn from_config(config_path: PathBuf, config: Config, root: PathBuf) -> Result<Self> {
        let config_path = normalized_path(&config_path)?;
        let root = normalized_path(&root)?;
        let mut digest = Sha256::new();
        // 路径标识不包含密钥内容；协议代次与路径字段分隔，避免拼接歧义。
        digest.update(b"wx-cli-runtime-v2\0");
        for path in [&config_path, &config.db_dir, &config.keys_file, &root] {
            let path = normalized_path(path)?;
            digest.update(path.to_string_lossy().to_lowercase().as_bytes());
            digest.update([0]);
        }
        let id = format!("{:x}", digest.finalize());
        let directory = root.join("accounts").join(&id);
        Ok(Self {
            config,
            config_path,
            root,
            id,
            directory,
        })
    }

    pub fn pipe_name(&self) -> String {
        format!("wx-cli-v2-{}", self.id)
    }
    pub fn pid_path(&self) -> PathBuf {
        self.directory.join("daemon.pid")
    }
    pub fn log_path(&self) -> PathBuf {
        self.directory.join("daemon.log")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.directory.join("cache")
    }
    pub fn mtime_file(&self) -> PathBuf {
        self.cache_dir().join("_mtimes.json")
    }

    /// 不共享文件句柄形成跨进程锁；锁文件可以保留，进程退出后锁自动释放。
    pub fn lock(&self, name: &str) -> Result<fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        fs::create_dir_all(&self.directory)?;
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(self.directory.join(name))
            .context("当前账号的运行锁暂不可用")
    }
}

/// 已存在部分解析目录联接，缺失部分保持绝对路径；创建目录前后身份应一致。
fn normalized_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.clone();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(
            ancestor
                .file_name()
                .context("无法解析账号运行路径")?
                .to_os_string(),
        );
        anyhow::ensure!(ancestor.pop(), "无法定位账号运行路径的祖先目录");
    }
    let mut normalized = ancestor.canonicalize()?;
    for part in tail.into_iter().rev() {
        normalized.push(part);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(account: &str, workspace: &str, home: &str) -> RuntimeContext {
        let base = std::env::temp_dir().join("wx-runtime-unit");
        RuntimeContext::from_config(
            base.join(workspace).join("config.json"),
            Config {
                db_dir: base.join(account).join("db_storage"),
                keys_file: base.join(workspace).join("all_keys.json"),
                decrypted_dir: base.join(workspace).join("decrypted"),
                wechat_process: String::new(),
            },
            base.join(home),
        )
        .unwrap()
    }

    #[test]
    fn separates_accounts_workspaces_and_runtime_roots() {
        let a = context("a", "work", "home");
        for b in [
            context("b", "work", "home"),
            context("a", "other", "home"),
            context("a", "work", "other-home"),
        ] {
            assert_ne!(a.id, b.id);
            assert_ne!(a.pipe_name(), b.pipe_name());
            assert_ne!(a.cache_dir(), b.cache_dir());
            assert_ne!(a.pid_path(), b.pid_path());
            assert_ne!(a.log_path(), b.log_path());
        }
        assert_eq!(a.id, context("a", "work", "home").id);
    }

    #[test]
    fn windows_path_case_does_not_change_identity() {
        assert_eq!(
            context("account", "work", "home").id,
            context("ACCOUNT", "WORK", "HOME").id
        );
    }

    #[test]
    fn identity_survives_directory_creation_and_lock_is_exclusive() {
        let base = std::env::temp_dir().join(format!(
            "wx-runtime-lock-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let cfg = Config {
            db_dir: base.join("db"),
            keys_file: base.join("keys.json"),
            decrypted_dir: base.join("out"),
            wechat_process: String::new(),
        };
        let a =
            RuntimeContext::from_config(base.join("config.json"), cfg.clone(), base.join("home"))
                .unwrap();
        let lock = a.lock("test.lock").unwrap();
        assert!(a.lock("test.lock").is_err());
        let b =
            RuntimeContext::from_config(base.join("config.json"), cfg, base.join("home")).unwrap();
        assert_eq!(a.id, b.id);
        drop(lock);
        drop(a.lock("test.lock").unwrap());
        fs::remove_dir_all(base).unwrap();
    }
}
