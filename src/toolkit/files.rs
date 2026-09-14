//! 批处理共享的路径边界、目录遍历和同卷原子文件发布。

use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

// 先解析已存在的祖先目录及目录联接，再检查源目录和输出目录是否重叠。
pub(super) fn resolved(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.components().any(|c| c == Component::ParentDir),
        "Parent-directory components are not allowed: {}",
        path.display()
    );
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.clone();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(
            ancestor
                .file_name()
                .context("Cannot resolve path")?
                .to_os_string(),
        );
        ensure!(ancestor.pop(), "Cannot resolve path");
    }
    let mut result = ancestor.canonicalize()?;
    for name in tail.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}

pub(crate) fn separate(source: &Path, output: &Path) -> Result<()> {
    let source = resolved(source)?.to_string_lossy().to_lowercase();
    let output = resolved(output)?.to_string_lossy().to_lowercase();
    ensure!(
        source != output && !output.starts_with(&format!("{}\\", source.trim_end_matches('\\'))),
        "Output must be outside the source directory"
    );
    Ok(())
}

pub(super) fn collect(root: &Path, extension: &str, skip_migrate: bool) -> Result<Vec<PathBuf>> {
    fn walk(
        base: &Path,
        root: &Path,
        ext: &str,
        skip: bool,
        files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            // 不跟随目录联接或重解析点，避免越过账号边界或陷入目录循环。
            use std::os::windows::fs::MetadataExt;
            if fs::symlink_metadata(&path)?.file_attributes() & 0x400 != 0 {
                continue;
            }
            if entry.file_type()?.is_dir() {
                if skip && root == base && entry.file_name().eq_ignore_ascii_case("migrate") {
                    continue;
                }
                walk(base, &path, ext, skip, files)?;
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, extension, skip_migrate, &mut files)?;
    files.sort();
    Ok(files)
}

pub(crate) fn atomic_output(path: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let path = std::path::absolute(path)?;
    let before = fingerprint(&path)?;
    publish(&path, &[], before, write)
}

/// Account resources remain protected even when their final files do not exist yet.
pub(crate) fn export_protected(runtime: &crate::runtime::RuntimeContext) -> Vec<PathBuf> {
    let mut paths = vec![
        runtime.config_path.clone(),
        runtime.config.keys_file.clone(),
        runtime.config.db_dir.clone(),
        runtime.config.decrypted_dir.clone(),
        runtime.directory.clone(),
    ];
    paths.extend(runtime.config.key_store.iter().cloned());
    paths.extend(
        runtime
            .config
            .key_store
            .iter()
            .map(|path| path.with_extension("key-update.lock")),
    );
    if let Some(parent) = runtime.config_path.parent() {
        paths.push(parent.join("account_key.dpapi"));
    }
    paths.retain(|path| !path.as_os_str().is_empty());
    paths
}

pub(crate) fn validate_export_target(
    runtime: &crate::runtime::RuntimeContext,
    path: &Path,
) -> Result<()> {
    validate_export_paths(path, &export_protected(runtime))
}

fn validate_export_paths(path: &Path, protected: &[PathBuf]) -> Result<()> {
    for input in protected {
        separate(input, path)?;
        separate(path, input)?;
    }
    if path.is_dir() {
        crate::attachment::local_files::HostOutputGuard::new(path)?.verify()?;
    } else {
        super::setup::check_target(path, protected)?;
    }
    Ok(())
}

/// A single artifact, not a multi-file transaction. Capture before querying/merging.
pub(crate) struct ExportTarget {
    path: PathBuf,
    protected: Vec<PathBuf>,
    before: Option<Fingerprint>,
}

impl ExportTarget {
    pub(crate) fn capture(runtime: &crate::runtime::RuntimeContext, path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let protected = export_protected(runtime);
        validate_export_paths(&path, &protected)?;
        let before = fingerprint(&path)?;
        Ok(Self {
            path,
            protected,
            before,
        })
    }

    pub(crate) fn write_json(self, value: &serde_json::Value) -> Result<()> {
        publish(&self.path, &self.protected, self.before, |temporary| {
            let file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(temporary)?;
            let mut writer = std::io::BufWriter::new(file);
            serde_json::to_writer_pretty(&mut writer, value)?;
            std::io::Write::flush(&mut writer)?;
            Ok(())
        })
    }

    pub(crate) fn write_bytes(self, bytes: &[u8]) -> Result<()> {
        publish(&self.path, &self.protected, self.before, |temporary| {
            Ok(fs::write(temporary, bytes)?)
        })
    }
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    identity: (u32, u32, u32),
    size: u64,
    digest: [u8; 32],
}

fn file_identity(file: &fs::File) -> Result<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(
        info.nNumberOfLinks == 1 && info.dwFileAttributes & 0x410 == 0,
        "Output is not a regular unlinked file"
    );
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

// Unlike configuration Snapshot, exports have no 16 MiB JSON ceiling. Hash in chunks.
fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    use sha2::{Digest, Sha256};
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    super::setup::check_target(path, &[])?;
    // Existing targets have an existing parent; pin it throughout the streaming read.
    let _parent = if path.try_exists()? {
        Some(crate::attachment::local_files::HostOutputGuard::new(
            path.parent().context("Output has no parent")?,
        )?)
    } else {
        None
    };
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let identity = file_identity(&file)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
        size += n as u64;
    }
    Ok(Some(Fingerprint {
        identity,
        size,
        digest: hash.finalize().into(),
    }))
}

fn publish(
    path: &Path,
    protected: &[PathBuf],
    before: Option<Fingerprint>,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    super::setup::check_target(path, protected)?;
    let guard = super::setup::parent_guard(path)?;
    // Attribute-only directory handles can permit rename on Windows. Request list/read
    // access as well, and deny DELETE sharing for the full callback and publication.
    let directory = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .custom_flags(0x02200000)
        .open(guard.output_root())?;
    ensure!(
        directory.metadata()?.is_dir() && directory.metadata()?.file_attributes() & 0x400 == 0,
        "Invalid output parent"
    );
    guard.verify()?;
    let temporary = tempfile::Builder::new()
        .prefix(".wx-export-")
        .suffix(".tmp")
        .tempfile_in(guard.output_root())?;
    super::private_file::restrict(temporary.as_file())?;
    // SQLite writers can leave sidecars; cleanup remains local to our temporary basename.
    struct Sidecars(PathBuf);
    impl Drop for Sidecars {
        fn drop(&mut self) {
            for suffix in ["-wal", "-shm"] {
                let _ = fs::remove_file(PathBuf::from(format!("{}{suffix}", self.0.display())));
            }
        }
    }
    let _sidecars = Sidecars(temporary.path().into());
    // Domain writers such as full_decrypt atomically replace their own staging file.
    // Close our write handle first, as crypto::atomic::Output::transform does.
    let temporary = temporary.into_temp_path();
    write(&temporary)?;
    guard.verify_replaceable_file(&temporary)?;
    let completed = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(3)
        .custom_flags(0x00200000)
        .open(&temporary)?;
    let temporary_identity = file_identity(&completed)?;
    super::private_file::restrict(&completed)?;
    completed.sync_all()?;
    drop(completed);
    guard.verify_replaceable_file(&temporary)?;
    ensure!(
        file_identity(&fs::File::open(&temporary)?)? == temporary_identity,
        "Temporary output identity changed"
    );
    super::setup::check_target(path, protected)?;
    ensure!(
        same_file::Handle::from_file(directory.try_clone()?)?
            == same_file::Handle::from_path(guard.output_root())?,
        "Output parent identity changed"
    );
    ensure!(
        fingerprint(path)? == before,
        "Output changed during export; existing file preserved"
    );
    guard.verify_replaceable_file(path)?;
    if before.is_some() {
        temporary.persist(path).map_err(|error| error.error)?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)?;
    }
    Ok(())
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use crate::{config::Config, runtime::RuntimeContext};

    fn runtime(root: &Path) -> RuntimeContext {
        RuntimeContext {
            config: Config {
                key_store: Some(root.join("private/store.dpapi")),
                db_dir: root.join("db"),
                keys_file: root.join("legacy/keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.into(),
            directory: root.join("runtime"),
            id: "export-fixture".into(),
        }
    }

    #[test]
    fn protects_existing_future_and_ancestor_account_paths_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime(dir.path());
        for path in export_protected(&runtime) {
            assert!(
                ExportTarget::capture(&runtime, &path).is_err(),
                "{}",
                path.display()
            );
            assert!(validate_export_target(&runtime, &path.join("nested.json")).is_err());
        }
        assert!(validate_export_target(&runtime, dir.path()).is_err());
        assert!(validate_export_target(&runtime, &dir.path().join("private")).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        let output = dir.path().join("artifacts/new.json");
        ExportTarget::capture(&runtime, &output)
            .unwrap()
            .write_bytes(b"synthetic")
            .unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"synthetic");
        crate::toolkit::private_file::assert_private_acl(&output);
    }

    #[test]
    fn rejects_protected_hardlink_alias_and_preserves_original() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime(dir.path());
        let store = runtime.config.key_store.as_ref().unwrap();
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::write(store, b"synthetic protected bytes").unwrap();
        let alias = dir.path().join("alias.json");
        fs::hard_link(store, &alias).unwrap();
        assert!(ExportTarget::capture(&runtime, &alias).is_err());
        assert_eq!(fs::read(store).unwrap(), b"synthetic protected bytes");
    }

    #[test]
    fn concurrent_change_and_creation_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime(dir.path());
        let output = dir.path().join("result.json");
        for existing in [false, true] {
            if existing {
                fs::write(&output, b"initial___").unwrap();
            }
            let target = ExportTarget::capture(&runtime, &output).unwrap();
            fs::write(&output, b"concurrent").unwrap();
            assert!(target.write_bytes(b"replacement").is_err());
            assert_eq!(fs::read(&output).unwrap(), b"concurrent");
            fs::remove_file(&output).unwrap();
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn failed_writer_and_locked_publication_preserve_previous_bytes() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.json");
        fs::write(&output, b"old").unwrap();
        assert!(atomic_output(&output, |temporary| {
            fs::write(temporary, b"partial")?;
            anyhow::bail!("synthetic writer failure")
        })
        .is_err());
        let target = ExportTarget::capture(&runtime(dir.path()), &output).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&output)
            .unwrap();
        assert!(target.write_bytes(b"new").is_err());
        drop(lock);
        assert_eq!(fs::read(&output).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn nested_atomic_writer_can_replace_stage_without_loosening_final_checks() {
        use std::{io::Write, os::windows::fs::OpenOptionsExt};
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("database.db");
        fs::write(&output, b"previous database").unwrap();
        atomic_output(&output, |stage| {
            // The same share mode used by crypto's atomic output snapshot.
            let old = fs::OpenOptions::new()
                .read(true)
                .share_mode(5)
                .open(stage)?;
            let mut replacement = tempfile::NamedTempFile::new_in(stage.parent().unwrap())?;
            replacement.write_all(b"verified replacement")?;
            replacement.as_file().sync_all()?;
            drop(old);
            replacement.persist(stage).map_err(|error| error.error)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"verified replacement");
        crate::toolkit::private_file::assert_private_acl(&output);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let source = dir.path().join("source.db");
        fs::write(&source, b"protected source").unwrap();
        assert!(atomic_output(&output, |stage| {
            fs::remove_file(stage)?;
            fs::hard_link(&source, stage)?;
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(&source).unwrap(), b"protected source");
        assert_eq!(fs::read(&output).unwrap(), b"verified replacement");
    }

    #[test]
    fn exports_above_configuration_snapshot_limit_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime(dir.path());
        let output = dir.path().join("large.json");
        let bytes = vec![b'x'; 17 * 1024 * 1024];
        ExportTarget::capture(&runtime, &output)
            .unwrap()
            .write_bytes(&bytes)
            .unwrap();
        ExportTarget::capture(&runtime, &output)
            .unwrap()
            .write_json(&serde_json::json!({"complete":true}))
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&output).unwrap()).unwrap()
                ["complete"],
            true
        );
    }

    #[test]
    fn parent_is_pinned_and_late_hardlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("output");
        fs::create_dir(&parent).unwrap();
        let protected = dir.path().join("protected");
        fs::write(&protected, b"keep").unwrap();
        let output = parent.join("result.json");
        assert!(atomic_output(&output, |temporary| {
            assert!(fs::rename(&parent, dir.path().join("moved")).is_err());
            fs::write(temporary, b"new")?;
            fs::hard_link(&protected, &output)?;
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(&protected).unwrap(), b"keep");
        assert_eq!(fs::read(&output).unwrap(), b"keep");
    }
}
