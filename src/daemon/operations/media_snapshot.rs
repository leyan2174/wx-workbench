//! Account-bound temporary snapshots for chat media export.
use crate::{
    adapters::wechat::media::voice::{self as database_media, DecryptedSource},
    daemon::cache::DbCache,
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};
use zeroize::Zeroize;

pub struct DatabaseMaterials(HashMap<String, String>);

impl DatabaseMaterials {
    pub fn new(keys: HashMap<String, String>) -> Self {
        Self(keys)
    }
}

impl std::fmt::Debug for DatabaseMaterials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DatabaseMaterials([REDACTED])")
    }
}

impl Drop for DatabaseMaterials {
    fn drop(&mut self) {
        for (mut source, mut key) in self.0.drain() {
            source.zeroize();
            key.zeroize();
        }
    }
}

pub struct Snapshot {
    account_id: String,
    sources: Vec<DecryptedSource>,
    // 私有快照存活至批次对象释放，避免共享 daemon 缓存被并发更新。
    _directory: tempfile::TempDir,
}

impl Snapshot {
    /// 调用者须保持 Snapshot 存活；切勿只保存路径后释放所有者。
    pub fn sources(&self) -> &[DecryptedSource] {
        &self.sources
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn planning_inputs(
        &self,
    ) -> Result<(&Path, crate::adapters::wechat::planning::PlanDatabases)> {
        use crate::adapters::wechat::planning::PlanDatabases;
        let root = self._directory.path();
        let mut inputs = PlanDatabases {
            message: Vec::new(),
            resource: None,
            media: Vec::new(),
        };
        for source in &self.sources {
            let relative = source
                .path
                .strip_prefix(root)
                .context("Snapshot path escaped private root")?
                .to_owned();
            ensure!(
                relative.components().count() == 1,
                "Invalid snapshot file path"
            );
            let name = source.source.replace('\\', "/").to_ascii_lowercase();
            if name == "message/message_resource.db"
                || name.starts_with("message/message_resource_")
            {
                ensure!(
                    inputs.resource.is_none(),
                    "Multiple resource databases are not supported by plan collection"
                );
                inputs.resource = Some(relative);
            } else if name.starts_with("message/message_") {
                inputs.message.push(relative);
            } else if name.starts_with("message/media_") {
                inputs.media.push(relative);
            }
        }
        Ok((root, inputs))
    }
}

#[derive(PartialEq, Eq)]
struct SourceState {
    path: PathBuf,
    identity: same_file::Handle,
    length: u64,
    modified: SystemTime,
}

fn states(paths: &[PathBuf]) -> Result<Vec<SourceState>> {
    let mut result = Vec::new();
    for path in paths {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let path = PathBuf::from(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() =>
                {
                    continue
                }
                Err(error) => return Err(error.into()),
            };
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "invalid database source"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "reparse database source"
                );
            }
            result.push(SourceState {
                identity: same_file::Handle::from_path(&path)?,
                path,
                length: metadata.len(),
                modified: metadata.modified()?,
            });
        }
    }
    Ok(result)
}

fn verify_sources(root: &Path, paths: &[PathBuf], before: &[SourceState]) -> Result<()> {
    ensure!(
        paths == snapshot_source_files(root)? && before == states(paths)?,
        "account database changed while preparing snapshot; retry batch"
    );
    Ok(())
}

/// 当前账号消息、媒体、resource、contact来源的私有只读快照。
pub fn prepare_snapshot(
    runtime: &RuntimeContext,
    mut supplied: DatabaseMaterials,
) -> Result<Snapshot> {
    ensure!(
        !runtime.id.trim().is_empty() && runtime.directory.is_absolute(),
        "fixed account context required"
    );
    // 独立线程运行异步解密，允许同步回调由现有 Tokio 宿主调用。
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<Snapshot> {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                executor.block_on(async {
                    let paths = snapshot_source_files(&runtime.config.db_dir)?;
                    let before = states(&paths)?;
                    let root = runtime.config.db_dir.canonicalize()?;
                    let mut normalized = DatabaseMaterials::new(HashMap::new());
                    for (mut original_source, mut key) in supplied.0.drain() {
                        let source = original_source.replace('\\', "/").to_ascii_lowercase();
                        original_source.zeroize();
                        if source.starts_with("message/message_")
                            || source.starts_with("message/media_")
                            || source == "contact/contact.db"
                        {
                            ensure!(
                                normalized.0.insert(source, key).is_none(),
                                "duplicate database key source"
                            );
                        } else {
                            key.zeroize();
                        }
                    }
                    let mut selected = DatabaseMaterials::new(HashMap::new());
                    for path in &paths {
                        let source = path
                            .strip_prefix(&root)?
                            .to_str()
                            .context("non-UTF8 source")?
                            .replace('\\', "/");
                        let key = normalized
                            .0
                            .remove(&source.to_ascii_lowercase())
                            .context("database source has no account key")?;
                        selected.0.insert(source, key);
                    }
                    ensure!(
                        normalized.0.is_empty(),
                        "account key inventory contains missing database sources"
                    );
                    fs::create_dir_all(&runtime.directory)?;
                    let directory = tempfile::Builder::new()
                        .prefix("export-snapshot-")
                        .tempdir_in(&runtime.directory)?;
                    let db = DbCache::with_dirs(
                        root,
                        directory.path().to_owned(),
                        directory.path().join("_mtimes.json"),
                        std::mem::take(&mut selected.0),
                    )
                    .await?;
                    let mut sources = Vec::new();
                    for source in db.raw_db_keys() {
                        let path = db
                            .get(&source)
                            .await?
                            .context("database snapshot missing")?;
                        sources.push(DecryptedSource { source, path });
                    }
                    verify_sources(&runtime.config.db_dir, &paths, &before)?;
                    Ok(Snapshot {
                        account_id: runtime.id.clone(),
                        sources,
                        _directory: directory,
                    })
                })
            })
            .join()
            .map_err(|_| anyhow::anyhow!("database snapshot worker panicked"))?
    })
}

fn snapshot_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = database_media::source_files(root)?;
    let directory = root.canonicalize()?.join("message");
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        ensure!(index < 4096, "database directory entry limit exceeded");
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let resource = name == "message_resource.db"
            || name
                .strip_prefix("message_resource_")
                .and_then(|s| s.strip_suffix(".db"))
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if resource {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut identities = std::collections::HashSet::new();
    for path in &paths {
        ensure!(
            identities.insert(same_file::Handle::from_path(path)?),
            "snapshot sources alias the same file"
        );
    }
    // 与主库同样拒绝资源库重解析点；这一步不打开 SQLite，也不解密。
    let _ = states(&paths)?;
    Ok(paths)
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn planning_mapping_is_case_insensitive_but_keeps_physical_paths() {
        let directory = tempfile::tempdir().unwrap();
        let names = [
            ("Message\\Message_0.DB", "AA.db"),
            ("MESSAGE/MEDIA_2.db", "Bb.db"),
            ("Message/Message_Resource.DB", "CC.db"),
            ("Contact/Contact.db", "DD.db"),
        ];
        let sources = names
            .iter()
            .map(|(source, physical)| DecryptedSource {
                source: (*source).into(),
                path: directory.path().join(physical),
            })
            .collect();
        let mut snapshot = Snapshot {
            account_id: "synthetic".into(),
            sources,
            _directory: directory,
        };
        let (_, inputs) = snapshot.planning_inputs().unwrap();
        assert_eq!(inputs.message, vec![PathBuf::from("AA.db")]);
        assert_eq!(inputs.media, vec![PathBuf::from("Bb.db")]);
        assert_eq!(inputs.resource, Some(PathBuf::from("CC.db")));
        snapshot.sources.push(DecryptedSource {
            source: "MESSAGE/MESSAGE_RESOURCE_1.DB".into(),
            path: snapshot._directory.path().join("EE.db"),
        });
        assert!(snapshot.planning_inputs().is_err());
    }

    #[test]
    fn planning_rejects_a_physical_path_outside_owned_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let snapshot = Snapshot {
            account_id: "synthetic".into(),
            sources: vec![DecryptedSource {
                source: "message/message_0.db".into(),
                path: outside.path().join("old.db"),
            }],
            _directory: directory,
        };
        assert!(snapshot.planning_inputs().is_err());
    }

    #[test]
    fn source_content_inventory_and_sidecar_changes_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("message")).unwrap();
        let path = root.path().join("message/message_0.db");
        fs::write(&path, b"synthetic-before").unwrap();
        let paths = snapshot_source_files(root.path()).unwrap();
        let before = states(&paths).unwrap();
        verify_sources(root.path(), &paths, &before).unwrap();
        fs::write(&path, b"synthetic-after-with-different-length").unwrap();
        assert!(verify_sources(root.path(), &paths, &before).is_err());
        let before = states(&paths).unwrap();
        fs::write(
            root.path().join("message/message_0.db-wal"),
            b"synthetic-wal",
        )
        .unwrap();
        assert!(verify_sources(root.path(), &paths, &before).is_err());
        let before = states(&paths).unwrap();
        fs::write(
            root.path().join("message/Message_Resource.DB"),
            b"synthetic-resource",
        )
        .unwrap();
        assert!(verify_sources(root.path(), &paths, &before).is_err());
    }

    #[test]
    fn old_shared_decrypted_cache_never_substitutes_for_account_materials() {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            key_store: None,
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("foreign-cache"),
            wechat_process: "SyntheticNotRunning.exe".into(),
        };
        fs::create_dir_all(config.db_dir.join("message")).unwrap();
        fs::write(
            config.db_dir.join("message/message_0.db"),
            b"synthetic-encrypted-source",
        )
        .unwrap();
        fs::create_dir_all(config.decrypted_dir.join("message")).unwrap();
        let old = config.decrypted_dir.join("message/message_0.db");
        fs::write(&old, b"foreign-account-cache-must-not-be-read").unwrap();
        let config_path = root.path().join("config.json");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let runtime =
            RuntimeContext::from_config(config_path, config, root.path().join("home")).unwrap();
        assert!(prepare_snapshot(&runtime, DatabaseMaterials::new(HashMap::new())).is_err());
        assert_eq!(
            fs::read(old).unwrap(),
            b"foreign-account-cache-must-not-be-read"
        );
    }
}
