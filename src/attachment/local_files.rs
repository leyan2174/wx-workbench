//! 共用的 Windows 本地路径句柄防护，不依赖图片、数据库或音频模块。

use anyhow::{bail, ensure, Context, Result};
use std::fs::{self, File, Metadata, OpenOptions};
#[cfg(test)]
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const MAX_ENTRIES: usize = 20_000;
const MAX_DIRECTORIES: usize = 1024;

/// 持有本地路径句柄，并在发布前重新核验原路径身份。
pub(crate) struct HostOutputGuard {
    output: Scan,
    output_root: PathBuf,
    inputs: Vec<Scan>,
    key_files: Vec<Pin>,
    future_inputs: Vec<PathBuf>,
}

impl HostOutputGuard {
    /// 此检查先于账号与密钥访问；空路径和相对路径在任何文件访问前失败。
    pub(crate) fn new(output_root: &Path) -> Result<Self> {
        let mut output = Scan::new();
        output.root(output_root)?;
        Ok(Self {
            output,
            output_root: output_root.into(),
            inputs: Vec::new(),
            key_files: Vec::new(),
            future_inputs: Vec::new(),
        })
    }

    pub(crate) fn output_root(&self) -> &Path {
        &self.output_root
    }

    /// 固定只读文件句柄，在守卫释放前禁止写入和删除该文件。
    pub(crate) fn pin_input(&mut self, path: &Path) -> Result<()> {
        self.protect(path)?;
        self.key_files.push(Pin::open(path, false)?);
        Ok(())
    }

    /// 必须在发布前复核，不能在提交成功后再追加可能失败的检查。
    pub(crate) fn verify(&self) -> Result<()> {
        for scan in std::iter::once(&self.output).chain(&self.inputs) {
            for pin in &scan.pins {
                pin.verify()?;
            }
        }
        for pin in &self.key_files {
            pin.verify()?;
        }
        for path in &self.future_inputs {
            self.future_scan(path)?;
        }
        Ok(())
    }

    /// 保护可能尚不存在的目录树，不创建目录；后续复核新出现的路径。
    pub(crate) fn protect_future(&mut self, path: &Path) -> Result<()> {
        ensure!(self.inputs.len() < 128, "protected input limit exceeded");
        let scan = self.future_scan(path)?;
        self.inputs.push(scan);
        self.future_inputs.push(path.into());
        Ok(())
    }

    fn future_scan(&self, path: &Path) -> Result<Scan> {
        ensure!(path.is_absolute(), "protected input must be absolute");
        ensure!(path.as_os_str().len() <= 32760, "protected path too long");
        let raw = path.to_str().context("invalid path encoding")?;
        ensure!(
            !raw.split(['/', '\\']).any(|s| s == "." || s == ".."),
            "relative path component"
        );
        let mut existing = path;
        loop {
            match fs::symlink_metadata(existing) {
                Ok(metadata) => {
                    ensure!(
                        stamp(&metadata)?.0,
                        "protected future path is not a directory"
                    );
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    safe_name(
                        existing
                            .file_name()
                            .and_then(|s| s.to_str())
                            .context("invalid protected input name")?,
                    )?;
                    existing = existing
                        .parent()
                        .context("protected input parent missing")?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut scan = Scan::new();
        scan.root(existing)?;
        let output_id = &self.output.pins.last().context("output root missing")?.id;
        ensure!(
            !scan.pins.iter().any(|pin| &pin.id == output_id),
            "protected input is inside output directory"
        );
        if existing == path {
            let input_id = &scan.pins.last().context("protected directory missing")?.id;
            ensure!(
                !self.output.pins.iter().any(|pin| &pin.id == input_id),
                "output overlaps protected directory"
            );
        }
        Ok(scan)
    }

    /// 检查输出目录的直属文件，不持续锁定需要原子替换的目标。
    /// 不能代替缓存读取器自身的文件身份固定及竞态检查。
    pub(crate) fn verify_replaceable_file(&self, path: &Path) -> Result<()> {
        self.verify()?;
        ensure!(path.is_absolute(), "replaceable file must be absolute");
        safe_name(
            path.file_name()
                .and_then(|s| s.to_str())
                .context("invalid replaceable filename")?,
        )?;
        let mut parent = Scan::new();
        parent.root(path.parent().context("replaceable parent missing")?)?;
        ensure!(
            parent.pins.last().context("replaceable parent missing")?.id
                == self.output.pins.last().context("output root missing")?.id,
            "replaceable file must be a direct output child"
        );
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(metadata) => ensure!(!stamp(&metadata)?.0, "replaceable target must be a file"),
        }
        let pin = Pin::open(path, false)?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            };
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            // 文件句柄在调用期间保持有效，输出缓冲区使用接口规定的结构体。
            unsafe {
                GetFileInformationByHandle(HANDLE(pin.file.as_raw_handle()), &mut info)?;
            }
            ensure!(
                info.nNumberOfLinks == 1,
                "multiply linked replaceable file rejected"
            );
        }
        pin.verify()
    }

    /// 目录不能与输出目录双向重叠，保护文件不能位于输出目录内。
    pub(crate) fn protect(&mut self, input: &Path) -> Result<()> {
        ensure!(self.inputs.len() < 128, "protected input limit exceeded");
        ensure!(input.is_absolute(), "protected input must be absolute");
        let parent = input.parent().context("protected input parent missing")?;
        safe_name(
            input
                .file_name()
                .and_then(|s| s.to_str())
                .context("invalid protected input name")?,
        )?;
        let mut guard = Scan::new();
        guard.root(parent)?;
        let directory = match fs::symlink_metadata(input) {
            Ok(metadata) => stamp(&metadata)?.0,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let output_id = &self.output.pins.last().context("output root missing")?.id;
        if directory {
            guard.directory(input)?;
            let source_id = &guard.pins.last().context("protected directory missing")?.id;
            ensure!(
                !self.output.pins.iter().any(|pin| &pin.id == source_id),
                "output overlaps protected directory"
            );
        }
        ensure!(
            !guard.pins.iter().any(|pin| &pin.id == output_id),
            "protected input is inside output directory"
        );
        self.inputs.push(guard);
        Ok(())
    }

    /// 仅读取已隔离的显式文件，缓冲区在成功和失败路径都会清零。
    #[cfg(test)]
    pub(crate) fn read_key_file(&mut self, path: &Path) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        const LIMIT: u64 = 4096;
        self.protect(path)?;
        let pin = Pin::open(path, false)?;
        ensure!(pin.stamp.1 <= LIMIT, "image key file exceeds byte limit");
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        (&pin.file).take(LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= LIMIT && bytes.len() as u64 == pin.stamp.1,
            "image key file changed or exceeded byte limit"
        );
        pin.verify()?;
        self.key_files.push(pin);
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stamp(pub(super) bool, pub(super) u64, SystemTime);

fn stamp(meta: &Metadata) -> Result<Stamp> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "reparse point rejected"
        );
    }
    ensure!(!meta.file_type().is_symlink(), "symlink rejected");
    ensure!(meta.is_file() || meta.is_dir(), "nonregular path rejected");
    Ok(Stamp(meta.is_dir(), meta.len(), meta.modified()?))
}

pub(crate) struct Pin {
    pub(super) path: PathBuf,
    pub(super) file: File,
    pub(super) id: same_file::Handle,
    pub(super) stamp: Stamp,
}

impl Pin {
    pub(crate) fn open(path: &Path, directory: bool) -> Result<Self> {
        let before = stamp(&fs::symlink_metadata(path)?)?;
        ensure!(before.0 == directory, "unexpected path type");
        let mut options = OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 固定源句柄，禁止并发写/删除，不跟随重解析点。
            options
                .access_mode(if directory { 0x80 } else { 0x80000000 })
                .share_mode(1)
                .custom_flags(0x02200000);
        }
        #[cfg(not(windows))]
        bail!("local files require Windows pinned file handles");
        let file = options.open(path)?;
        let current = stamp(&file.metadata()?)?;
        ensure!(
            before.0 == current.0 && (directory || before == current),
            "source changed while opening"
        );
        let pin = Self {
            path: path.into(),
            id: same_file::Handle::from_file(file.try_clone()?)?,
            file,
            stamp: current,
        };
        pin.verify()?;
        Ok(pin)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        let current = stamp(&fs::symlink_metadata(&self.path)?)?;
        ensure!(
            self.id == same_file::Handle::from_path(&self.path)?,
            "path identity changed"
        );
        ensure!(
            self.stamp.0 == current.0 && (current.0 || self.stamp == current),
            "source changed"
        );
        Ok(())
    }
}

pub(super) fn safe_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name != "." && name != "..",
        "invalid path component"
    );
    ensure!(
        !name.ends_with([' ', '.'])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "unsafe path component"
    );
    let stem = name.split('.').next().unwrap().to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|s| {
            matches!(
                s,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !numbered_device,
        "device path rejected"
    );
    Ok(())
}

pub(super) struct Scan {
    pub(super) pins: Vec<Pin>,
    snapshots: Vec<(PathBuf, Vec<(String, Stamp)>)>,
    missing: Vec<PathBuf>,
    left: usize,
}

impl Scan {
    pub(super) fn new() -> Self {
        Self {
            pins: Vec::new(),
            snapshots: Vec::new(),
            missing: Vec::new(),
            left: MAX_ENTRIES,
        }
    }

    pub(super) fn root(&mut self, path: &Path) -> Result<()> {
        ensure!(
            path.is_absolute() && path.as_os_str().len() <= 32760,
            "explicit absolute root required"
        );
        let raw = path.to_str().context("invalid path encoding")?;
        ensure!(
            !raw.split(['/', '\\']).any(|s| s == "." || s == ".."),
            "relative path component"
        );
        let mut current = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    safe_name(name.to_str().context("invalid path encoding")?)?
                }
                #[cfg(windows)]
                Component::Prefix(p) => {
                    ensure!(
                        matches!(
                            p.kind(),
                            std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                        ),
                        "only local disk paths supported"
                    );
                    current.push(component);
                    continue;
                }
                Component::RootDir => {}
                _ => bail!("invalid root component"),
            }
            current.push(component);
            self.directory(&current)?;
        }
        Ok(())
    }

    pub(super) fn directory(&mut self, path: &Path) -> Result<()> {
        ensure!(
            self.pins.len() < MAX_DIRECTORIES,
            "directory limit exceeded"
        );
        self.pins.push(Pin::open(path, true)?);
        Ok(())
    }

    pub(super) fn optional_directory(&mut self, path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ensure!(
                    self.missing.len() < MAX_DIRECTORIES,
                    "missing directory limit exceeded"
                );
                self.missing.push(path.into());
                Ok(false)
            }
            Err(e) => Err(e.into()),
            Ok(_) => {
                self.directory(path)?;
                Ok(true)
            }
        }
    }

    pub(super) fn entries(&mut self, path: &Path) -> Result<Vec<(String, Stamp)>> {
        let entries = read_entries(path, &mut self.left)?;
        self.snapshots.push((path.into(), entries.clone()));
        Ok(entries)
    }

    pub(super) fn verify(&mut self) -> Result<()> {
        for pin in &self.pins {
            pin.verify()?;
        }
        for path in &self.missing {
            ensure!(
                matches!(fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
                "missing path changed"
            );
        }
        for (path, entries) in &self.snapshots {
            ensure!(
                *entries == read_entries(path, &mut self.left)?,
                "candidate directory changed"
            );
        }
        Ok(())
    }
}

pub(super) fn read_entries(path: &Path, left: &mut usize) -> Result<Vec<(String, Stamp)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        *left = left.checked_sub(1).context("entry limit exceeded")?;
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid filename"))?;
        safe_name(&name)?;
        entries.push((name, stamp(&fs::symlink_metadata(entry.path())?)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn future_directories_are_rechecked_without_creation() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let future = temp.path().join("future/cache");
        let mut guard = HostOutputGuard::new(&output).unwrap();
        guard.protect_future(&future).unwrap();
        assert!(!future.exists());
        guard.verify().unwrap();
        fs::create_dir_all(&future).unwrap();
        guard.verify().unwrap();
        assert!(guard.protect_future(&output.join("missing/deep")).is_err());
        assert!(guard.protect_future(temp.path()).is_err());
        assert!(guard
            .protect_future(&temp.path().join("bad/../path"))
            .is_err());
    }

    #[test]
    fn future_directory_rejects_new_junction() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let future = temp.path().join("future");
        let mut guard = HostOutputGuard::new(&output).unwrap();
        guard.protect_future(&future).unwrap();
        let result = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&future)
            .arg(&output)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(guard.verify().is_err());
        fs::remove_dir(&future).unwrap();
        assert!(output.is_dir());
    }

    #[test]
    fn replaceable_target_rejects_aliases_but_remains_unlocked() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let guard = HostOutputGuard::new(&output).unwrap();
        let target = output.join("cache.json");
        guard.verify_replaceable_file(&target).unwrap();
        let source = temp.path().join("model");
        fs::write(&source, b"model").unwrap();
        fs::hard_link(&source, &target).unwrap();
        assert!(guard.verify_replaceable_file(&target).is_err());
        fs::remove_file(&target).unwrap();
        fs::write(&target, b"cache").unwrap();
        guard.verify_replaceable_file(&target).unwrap();
        fs::write(&target, b"new cache").unwrap();
        assert!(guard.verify_replaceable_file(&source).is_err());
        assert!(guard.verify_replaceable_file(&output).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"model");
    }

    #[test]
    fn host_guard_pins_files_until_drop() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let input = temp.path().join("model.bin");
        fs::write(&input, b"model").unwrap();
        let mut guard = HostOutputGuard::new(&output).unwrap();
        assert_eq!(guard.output_root(), output);
        guard.pin_input(&input).unwrap();
        assert!(OpenOptions::new().write(true).open(&input).is_err());
        assert!(fs::remove_file(&input).is_err());
        for _ in 0..3 {
            guard.verify().unwrap();
        }
        drop(guard);
        fs::write(&input, b"updated").unwrap();
    }

    #[test]
    fn host_guard_protect_allows_future_file_and_atomic_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let cache = temp.path().join("cache.json");
        let mut guard = HostOutputGuard::new(&output).unwrap();
        guard.protect(&cache).unwrap();
        fs::write(&cache, b"old").unwrap();
        let mut replacement = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        std::io::Write::write_all(&mut replacement, b"new").unwrap();
        replacement.persist(&cache).unwrap();
        guard.verify().unwrap();
        assert_eq!(fs::read(&cache).unwrap(), b"new");
    }

    #[test]
    fn host_guard_rejects_overlap_and_missing_parent() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        let nested = output.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let mut guard = HostOutputGuard::new(&output).unwrap();
        assert!(guard.protect(temp.path()).is_err());
        assert!(guard.protect(&nested).is_err());
        assert!(guard.protect(&output.join("future")).is_err());
        assert!(guard.protect(&temp.path().join("missing/file")).is_err());
        assert!(guard.pin_input(&temp.path().join("missing")).is_err());
        assert!(guard.pin_input(&nested).is_err());
        guard.verify().unwrap();
    }

    #[test]
    fn host_guard_verify_checks_input_and_key_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        let input = temp.path().join("key");
        fs::write(&input, b"key").unwrap();
        let mut guard = HostOutputGuard::new(&output).unwrap();
        guard.read_key_file(&input).unwrap();
        guard.verify().unwrap();
        guard.key_files[0].stamp.1 += 1;
        assert!(guard.verify().is_err());
    }

    #[test]
    fn host_guard_verify_checks_directory_identity() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        let input = temp.path().join("input");
        fs::create_dir(&output).unwrap();
        fs::create_dir(&input).unwrap();
        let mut guard = HostOutputGuard::new(&output).unwrap();
        guard.protect(&input).unwrap();
        if fs::rename(&output, temp.path().join("moved")).is_ok() {
            assert!(guard.verify().is_err());
            return;
        }
        guard.inputs[0].pins.last_mut().unwrap().path = output;
        assert!(guard.verify().is_err());
    }
}
