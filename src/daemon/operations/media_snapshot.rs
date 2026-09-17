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

/// 账号级完整只读快照；私有解密产物由返回值持有。
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
                    ensure!(
                        paths == snapshot_source_files(&runtime.config.db_dir)?
                            && before == states(&paths)?,
                        "account database changed while preparing snapshot; retry batch"
                    );
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
