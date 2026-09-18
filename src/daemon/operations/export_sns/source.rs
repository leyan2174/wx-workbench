//! Per-export source handles; path identity is not account provenance.
use crate::attachment::local_files::{HostOutputGuard, Pin};
use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct Sources {
    directories: BTreeMap<PathBuf, HostOutputGuard>,
    files: Vec<Pin>,
    absent: Vec<PathBuf>,
}

impl Sources {
    pub(super) fn directory(&mut self, path: &Path) -> Result<PathBuf> {
        let path = std::path::absolute(path)?;
        if !self.directories.contains_key(&path) {
            self.directories
                .insert(path.clone(), HostOutputGuard::new(&path)?);
        }
        Ok(path)
    }

    fn file(&mut self, path: &Path) -> Result<PathBuf> {
        let path = std::path::absolute(path)?;
        self.directory(path.parent().context("Offline source has no parent")?)?;
        self.files.push(Pin::open(&path, false)?);
        // Resolve only after rejecting reparse ancestors and holding the source handle.
        Ok(path.canonicalize()?)
    }

    pub(super) fn database(&mut self, path: &Path) -> Result<PathBuf> {
        let path = self.file(path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            let sidecar = PathBuf::from(name);
            match std::fs::symlink_metadata(&sidecar) {
                Ok(_) => {
                    self.file(&sidecar)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.absent.push(sidecar)
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(path)
    }

    pub(super) fn candidate(&mut self, path: &Path, image: bool) -> Result<bool> {
        let path = self.file(path)?;
        if !image {
            return Ok(false);
        }
        let mut prefix = Vec::new();
        std::fs::File::open(path)?
            .take(6)
            .read_to_end(&mut prefix)?;
        Ok(prefix.as_slice() == crate::adapters::wechat::moments::cache::V2)
    }

    pub(super) fn verify(&self) -> Result<()> {
        for guard in self.directories.values() {
            guard.verify()?;
        }
        for file in &self.files {
            file.verify()?;
        }
        for path in &self.absent {
            ensure!(
                matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "Offline SQLite source changed; checkpoint and retry"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_and_sidecar_cannot_be_replaced_until_guards_drop() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sns.db");
        let sidecar = temp.path().join("sns.db-wal");
        std::fs::write(&path, b"synthetic sqlite source").unwrap();
        std::fs::write(&sidecar, b"synthetic sidecar").unwrap();
        let mut sources = Sources::default();
        sources.database(&path).unwrap();
        for file in [&path, &sidecar] {
            assert!(std::fs::write(file, b"replacement").is_err());
            assert!(std::fs::remove_file(file).is_err());
        }
        sources.verify().unwrap();
        drop(sources);
        std::fs::write(&path, b"released").unwrap();
        std::fs::remove_file(sidecar).unwrap();
    }

    #[test]
    fn newly_created_sidecar_requires_revalidation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sns.db");
        std::fs::write(&path, b"synthetic").unwrap();
        let mut sources = Sources::default();
        sources.database(&path).unwrap();
        std::fs::write(temp.path().join("sns.db-wal"), b"changed source").unwrap();
        assert!(sources.verify().is_err());
    }

    #[test]
    fn cache_root_is_pinned_for_operation_lifetime() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let mut sources = Sources::default();
        sources.directory(&root).unwrap();
        assert!(std::fs::rename(&root, temp.path().join("other")).is_err());
        sources.verify().unwrap();
        drop(sources);
        std::fs::rename(&root, temp.path().join("other")).unwrap();
    }
}
