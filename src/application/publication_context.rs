//! Short-lived publication context for account-bound and explicit offline operations.
use crate::{
    attachment::local_files::HostOutputGuard, runtime::RuntimeContext,
    service::config_pin::ConfigPin,
};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

enum Configuration {
    Present {
        runtime: Box<RuntimeContext>,
        pin: ConfigPin,
    },
    Missing {
        path: PathBuf,
        absent: Vec<PathBuf>,
        ancestor: HostOutputGuard,
    },
}

/// Missing configuration is observed from disk, never selected by a caller flag.
pub(crate) struct PublicationContext(Configuration);

impl PublicationContext {
    pub(crate) fn current() -> Result<Self> {
        let expected = match std::env::var("WX_CLI_EXPECTED_RUNTIME") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        Self::at_expected(
            &crate::config::find_config_file()?,
            crate::config::cli_dir(),
            expected.as_deref(),
        )
    }

    fn at_expected(path: &Path, root: PathBuf, expected: Option<&str>) -> Result<Self> {
        let context = Self::at(path, root)?;
        if let Some(expected) = expected {
            let runtime = context
                .runtime()
                .context("Expected account configuration is missing")?;
            ensure!(
                runtime.id == expected,
                "Account changed before export execution"
            );
        }
        Ok(context)
    }

    pub(crate) fn at(path: &Path, root: PathBuf) -> Result<Self> {
        let path = std::path::absolute(path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let config = crate::config::load_config_at(&path)?;
                let runtime = RuntimeContext::from_config(path, config, root)?;
                Self::for_runtime(&runtime)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut absent = vec![path.clone()];
                let mut parent = path.parent().context("Configuration has no parent")?;
                loop {
                    match fs::symlink_metadata(parent) {
                        Ok(_) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            absent.push(parent.to_owned());
                            parent = parent
                                .parent()
                                .context("Configuration has no existing ancestor")?;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                let ancestor = HostOutputGuard::new(parent)?;
                let context = Self(Configuration::Missing {
                    path,
                    ancestor,
                    absent,
                });
                context.verify()?;
                Ok(context)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn for_runtime(runtime: &RuntimeContext) -> Result<Self> {
        let pin = ConfigPin::new(runtime)?;
        let context = Self(Configuration::Present {
            runtime: Box::new(runtime.clone()),
            pin,
        });
        context.verify()?;
        Ok(context)
    }

    pub(crate) fn account(&self) -> Option<&RuntimeContext> {
        match &self.0 {
            Configuration::Present { runtime, .. } => Some(runtime),
            Configuration::Missing { .. } => None,
        }
    }

    pub(crate) fn runtime(&self) -> Result<&RuntimeContext> {
        self.account()
            .context("Operation defaults require an account configuration")
    }

    pub(crate) fn config_path(&self) -> &Path {
        match &self.0 {
            Configuration::Present { runtime, .. } => &runtime.config_path,
            Configuration::Missing { path, .. } => path,
        }
    }

    pub(crate) fn verify(&self) -> Result<()> {
        match &self.0 {
            Configuration::Present { runtime, pin } => pin.verify(runtime),
            Configuration::Missing {
                absent, ancestor, ..
            } => {
                ancestor.verify()?;
                for path in absent {
                    match fs::symlink_metadata(path) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                        Ok(_) => anyhow::bail!("Previously absent configuration path appeared"),
                    }
                }
                Ok(())
            }
        }
    }

    /// Pass these paths to ExportTarget; verify this context in its final callback.
    pub(crate) fn protected(&self, input: &Path) -> Result<Vec<PathBuf>> {
        let mut paths = match &self.0 {
            Configuration::Present { runtime, .. } => {
                crate::infrastructure::publication::export_protected(runtime)
            }
            Configuration::Missing { path, .. } => vec![path.clone()],
        };
        paths.push(std::path::absolute(input)?);
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_account_rejects_missing_or_changed_config_without_environment_mutation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let home = root.path().join("runtime");
        assert!(
            PublicationContext::at_expected(&path, home.clone(), Some("expected-account")).is_err()
        );
        assert!(PublicationContext::at_expected(&path, home.clone(), None)
            .unwrap()
            .account()
            .is_none());
        let config = crate::config::Config {
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            key_store: None,
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let runtime =
            RuntimeContext::from_config(path.clone(), config.clone(), home.clone()).unwrap();
        let context =
            PublicationContext::at_expected(&path, home.clone(), Some(&runtime.id)).unwrap();
        assert!(context.runtime().unwrap().same_account(&runtime).unwrap());
        drop(context);
        assert!(
            PublicationContext::at_expected(&path, home.clone(), Some("another-account")).is_err()
        );
        let mut changed = config;
        changed.keys_file = root.path().join("other-keys.json");
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(PublicationContext::at_expected(&path, home.clone(), Some(&runtime.id)).is_err());
        fs::remove_file(&path).unwrap();
        assert!(PublicationContext::at_expected(&path, home, Some(&runtime.id)).is_err());
    }

    #[test]
    fn missing_configuration_and_ancestors_must_remain_missing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("future/config.json");
        let context = PublicationContext::at(&path, root.path().join("runtime")).unwrap();
        assert!(context.account().is_none());
        assert!(context.runtime().is_err());
        fs::create_dir(path.parent().unwrap()).unwrap();
        assert!(context.verify().is_err());
        drop(context);
        let context = PublicationContext::at(&path, root.path().join("runtime")).unwrap();
        fs::write(&path, b"new configuration").unwrap();
        assert!(context.verify().is_err());
        drop(context);
        assert!(PublicationContext::at(&path, root.path().join("runtime")).is_err());
    }

    #[test]
    fn offline_protection_includes_the_future_config_and_input() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let input = root.path().join("input.dat");
        fs::write(&input, b"synthetic").unwrap();
        let context = PublicationContext::at(&path, root.path().join("runtime")).unwrap();
        let protected = context.protected(&input).unwrap();
        for target in [&path, &input] {
            assert!(
                crate::infrastructure::publication::ExportTarget::capture_paths(target, &protected)
                    .is_err()
            );
        }
        assert!(!path.exists());
        assert_eq!(fs::read(&input).unwrap(), b"synthetic");
    }
}
