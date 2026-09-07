//! 清理专用文件句柄：固定祖先路径，拒绝链接，只删除已核验的同一个文件对象。

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Component, Path, PathBuf, Prefix},
};
use windows::Win32::{
    Foundation::{BOOLEAN, HANDLE},
    Storage::FileSystem::{
        FileDispositionInfo, GetFileInformationByHandle, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, FILE_DISPOSITION_INFO,
    },
};

use crate::attachment::local_files::HostOutputGuard;

pub(super) const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub volume: u32,
    pub file_index: u64,
    pub bytes: u64,
    pub created: u64,
    pub modified: u64,
    pub attributes: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryIdentity {
    pub path: PathBuf,
    pub volume: u32,
    pub file_index: u64,
    pub created: u64,
}

pub(super) fn safe_component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name != "." && name != ".."
            && !name.ends_with([' ', '.'])
            && !name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "不安全的路径分量"
    );
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    let device_number = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
    ensure!(
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            && !device_number,
        "拒绝 Windows 设备名"
    );
    Ok(())
}

/// 逐层固定父目录后才检查下一层，不通过 junction 解析路径，也不使用 cwd。
pub(super) fn absolute(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute() && path.as_os_str().len() <= 32760, "必须提供显式绝对路径");
    let text = path.to_str().context("路径不是有效 Unicode")?;
    ensure!(!text.split(['/', '\\']).any(|c| c == "." || c == ".."), "路径不得包含相对分量");
    let mut components = path.components();
    let prefix = components.next().context("路径缺少盘符")?;
    ensure!(matches!(prefix, Component::Prefix(p) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))), "只接受本地盘符路径");
    ensure!(components.next() == Some(Component::RootDir), "路径缺少根目录");
    let names = components.map(|component| match component {
        Component::Normal(name) => {
            let text = name.to_str().context("路径不是有效 Unicode")?;
            safe_component(text)?;
            Ok(name.to_os_string())
        }
        _ => anyhow::bail!("无效的路径分量"),
    }).collect::<Result<Vec<_>>>()?;
    ensure!(names.len() <= 128, "路径层级超过限制");
    let mut current = PathBuf::new();
    current.push(prefix.as_os_str());
    current.push(Path::new("\\"));
    let mut guard = HostOutputGuard::new(&current)?;
    for (index, name) in names.iter().enumerate() {
        let parent = current.clone();
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                use std::os::windows::fs::MetadataExt;
                ensure!(meta.file_attributes() & 0x400 == 0 && !meta.file_type().is_symlink(), "拒绝链接或重解析点");
                if meta.is_dir() {
                    guard = HostOutputGuard::new(&current)?;
                } else {
                    ensure!(meta.is_file() && index + 1 == names.len(), "路径中包含非目录或特殊对象");
                    let pin = OpenOptions::new().access_mode(0x80).share_mode(1)
                        .custom_flags(0x00200000).open(&current)?;
                    information(&pin, false)?;
                    guard.verify()?;
                    // 固定文件后展开短文件名，与 RuntimeContext 的 canonicalize 身份算法一致。
                    return local_canonical(&current);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                guard.verify()?;
                let mut result = local_canonical(&parent)?;
                for tail in &names[index..] { result.push(tail); }
                return Ok(result);
            }
            Err(error) => return Err(error.into()),
        }
    }
    guard.verify()?;
    local_canonical(&current)
}

fn local_canonical(path: &Path) -> Result<PathBuf> {
    let result = path.canonicalize()?;
    ensure!(matches!(result.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))), "拒绝远程或设备路径");
    Ok(result)
}

fn information(file: &File, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // 句柄在整个调用期间存活，缓冲区类型和大小由 Win32 接口规定。
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?; }
    ensure!(info.dwFileAttributes & 0x400 == 0, "拒绝重解析点句柄");
    ensure!((info.dwFileAttributes & 0x10 != 0) == directory, "文件类型发生变化");
    if !directory { ensure!(info.nNumberOfLinks == 1, "拒绝多重硬链接文件"); }
    Ok(info)
}

fn fingerprint(info: BY_HANDLE_FILE_INFORMATION, sha256: String) -> Fingerprint {
    Fingerprint {
        volume: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        bytes: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32) | u64::from(info.ftCreationTime.dwLowDateTime),
        modified: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32) | u64::from(info.ftLastWriteTime.dwLowDateTime),
        attributes: info.dwFileAttributes,
        sha256,
    }
}

/// 不申请内容读取权限、不计算哈希；调用方必须保持父目录固定。
pub(super) fn metadata_only_file(path: &Path) -> Result<(u32, u64, u64)> {
    let file = OpenOptions::new().access_mode(0x80).share_mode(7)
        .custom_flags(0x00200000).open(path)?;
    let info = information(&file, false)?;
    Ok((info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow)))
}

pub(super) struct PinnedFile {
    file: File,
    guard: HostOutputGuard,
    path: PathBuf,
    deletable: bool,
}

impl PinnedFile {
    pub(super) fn open(path: &Path, deletable: bool) -> Result<Self> {
        let parent = path.parent().context("文件缺少父目录")?;
        let guard = HostOutputGuard::new(parent)?;
        guard.verify_replaceable_file(path)?;
        let file = OpenOptions::new()
            .access_mode(0x80000000 | if deletable { 0x00010000 } else { 0 })
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(path)
            .context("文件正在使用、不可读或不可独占删除")?;
        information(&file, false)?;
        guard.verify()?;
        Ok(Self { file, guard, path: path.into(), deletable })
    }

    pub(super) fn snapshot(&mut self) -> Result<Fingerprint> {
        self.guard.verify()?;
        let before = fingerprint(information(&self.file, false)?, String::new());
        ensure!(before.bytes <= MAX_FILE_BYTES, "单个文件超过清理计划读取上限");
        self.file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = zeroize::Zeroizing::new([0u8; 65536]);
        loop {
            let count = self.file.read(&mut buffer[..])?;
            if count == 0 { break; }
            bytes = bytes.checked_add(count as u64).context("文件长度溢出")?;
            ensure!(bytes <= before.bytes, "读取期间文件增长");
            hash.update(&buffer[..count]);
        }
        ensure!(bytes == before.bytes, "读取期间文件长度变化");
        let after = fingerprint(information(&self.file, false)?, String::new());
        ensure!(before == after, "读取期间文件身份或元数据变化");
        self.guard.verify()?;
        Ok(Fingerprint { sha256: format!("{:x}", hash.finalize()), ..before })
    }

    pub(super) fn json_bytes(&mut self) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let before = fingerprint(information(&self.file, false)?, String::new());
        ensure!(before.bytes <= MAX_JSON_BYTES, "JSON 清单超过读取上限");
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        (&mut self.file).take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 == before.bytes, "JSON 清单读取期间变化");
        ensure!(before == fingerprint(information(&self.file, false)?, String::new()), "JSON 清单身份变化");
        self.guard.verify()?;
        Ok(bytes)
    }

    pub(super) fn prefix(&mut self) -> Result<[u8; 16]> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = [0u8; 16];
        let mut read = 0;
        while read < bytes.len() {
            let n = self.file.read(&mut bytes[read..])?;
            if n == 0 { break; }
            read += n;
        }
        Ok(bytes)
    }

    pub(super) fn directories(&self) -> Result<Vec<DirectoryIdentity>> {
        let parent = self.path.parent().context("文件缺少父目录")?;
        let mut current = PathBuf::new();
        let mut result = Vec::new();
        for component in parent.components() {
            current.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) { continue; }
            let file = OpenOptions::new().access_mode(0x80).share_mode(1)
                .custom_flags(0x02200000).open(&current)?;
            let info = information(&file, true)?;
            result.push(DirectoryIdentity {
                path: current.clone(), volume: info.dwVolumeSerialNumber,
                file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
                created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32) | u64::from(info.ftCreationTime.dwLowDateTime),
            });
        }
        self.guard.verify()?;
        Ok(result)
    }

    pub(super) fn verify_directories(&self, expected: &[DirectoryIdentity]) -> Result<()> {
        ensure!(self.directories()? == expected, "文件祖先目录身份变化");
        Ok(())
    }

    /// 不按路径重新打开删除目标；处置标志仅作用于已校验并独占写/删除的句柄。
    pub(super) fn delete(mut self, expected: &Fingerprint) -> Result<()> {
        ensure!(self.deletable, "只读句柄不能用于删除");
        ensure!(&self.snapshot()? == expected, "删除前文件变化");
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: BOOLEAN(1) };
        unsafe {
            SetFileInformationByHandle(
                HANDLE(self.file.as_raw_handle()), FileDispositionInfo,
                &disposition as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )?;
        }
        // 其它共享读取句柄可能推迟物理空间回收，调用方报告的是删除请求而非回收量。
        drop(self.file);
        Ok(())
    }
}

/// 使用 RuntimeContext 相同的锁文件协议，加上不跟随链接及硬链接检查。
pub(super) fn runtime_locks(directory: &Path) -> Result<(HostOutputGuard, Vec<File>)> {
    let guard = HostOutputGuard::new(directory)?;
    let mut locks = Vec::new();
    for name in ["startup.lock", "daemon.lock", "web.lock", "cleanup.lock"] {
        let path = directory.join(name);
        guard.verify_replaceable_file(&path)?;
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .share_mode(0).custom_flags(0x00200000).open(&path)
            .context("当前账号后台、Web 服务或另一个清理操作仍在使用 runtime")?;
        information(&file, false)?;
        locks.push(file);
    }
    guard.verify()?;
    Ok((guard, locks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_only_reports_synthetic_binary_identity_and_length() -> Result<()> {
        let temp = tempfile::Builder::new().prefix("wx-cleanup-handle-test-").tempdir()?;
        let path = temp.path().join("unknown.blob");
        let bytes = [0xff, 0x00, 0x81, 0x17];
        fs::write(&path, bytes)?;
        let guard = HostOutputGuard::new(temp.path())?;
        let (volume, index, length) = metadata_only_file(&path)?;
        assert_eq!(length, bytes.len() as u64);
        let mut pin = PinnedFile::open(&path, false)?;
        let snapshot = pin.snapshot()?;
        assert_eq!((snapshot.volume, snapshot.file_index, snapshot.bytes), (volume, index, length));
        guard.verify()?;
        assert_eq!(fs::read(&path)?, bytes);
        Ok(())
    }

    #[test]
    fn metadata_and_deletion_handles_reject_multiple_hard_links() -> Result<()> {
        let temp = tempfile::Builder::new().prefix("wx-cleanup-handle-test-").tempdir()?;
        let path = temp.path().join("fixture.blob");
        let alias = temp.path().join("alias.blob");
        fs::write(&path, b"synthetic linked file")?;
        fs::hard_link(&path, &alias)?;
        assert!(metadata_only_file(&path).is_err());
        assert!(PinnedFile::open(&path, true).is_err());
        assert_eq!(fs::read(&path)?, b"synthetic linked file");
        assert_eq!(fs::read(&alias)?, b"synthetic linked file");
        Ok(())
    }

    #[test]
    fn read_only_handle_cannot_delete_fixture() -> Result<()> {
        let temp = tempfile::Builder::new().prefix("wx-cleanup-handle-test-").tempdir()?;
        let path = temp.path().join("fixture.blob");
        fs::write(&path, b"synthetic read-only fixture")?;
        let mut pin = PinnedFile::open(&path, false)?;
        let snapshot = pin.snapshot()?;
        assert!(pin.delete(&snapshot).is_err());
        assert_eq!(fs::read(&path)?, b"synthetic read-only fixture");
        Ok(())
    }

    #[test]
    fn stale_fingerprint_refuses_handle_deletion() -> Result<()> {
        let temp = tempfile::Builder::new().prefix("wx-cleanup-handle-test-").tempdir()?;
        let path = temp.path().join("fixture.blob");
        fs::write(&path, b"synthetic original")?;
        let mut original = PinnedFile::open(&path, false)?;
        let snapshot = original.snapshot()?;
        drop(original);
        fs::write(&path, b"synthetic modified fixture")?;
        let current = PinnedFile::open(&path, true)?;
        assert!(current.delete(&snapshot).is_err());
        assert_eq!(fs::read(&path)?, b"synthetic modified fixture");
        Ok(())
    }
}
