/// Windows WeChat 进程内存密钥扫描器
///
/// 使用 Windows API：
/// - CreateToolhelp32Snapshot + Process32Next: 枚举进程找 Weixin.exe
/// - OpenProcess: 获取进程句柄（需要 PROCESS_VM_READ | PROCESS_QUERY_INFORMATION）
/// - VirtualQueryEx: 枚举内存区域
/// - ReadProcessMemory: 读取内存内容
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

use super::KeyEntry;

pub(super) mod account;
mod config_cipher;
mod legacy;
mod version;

const MAX_SOURCE_ENTRIES: usize = 20_000;
const MAX_SOURCE_DIRECTORIES: usize = 1024;
const MAX_SOURCE_FILES: usize = 4096;
const MAX_DATABASES: usize = 2048;
const MAX_SOURCE_DEPTH: usize = 16;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WAL_FRAMES: usize = 100_000;

/// 只固定路径身份，不固定大小和修改时间，允许活动 SQLite 持续写入。
struct SourcePin {
    path: PathBuf,
    file: File,
    identity: (u32, u64),
    directory: bool,
}

impl SourcePin {
    fn open(path: &Path, directory: bool) -> Result<Self> {
        ensure!(source_metadata(path)? == directory, "库存路径类型不符");
        let mut options = OpenOptions::new();
        // 共享读写但不共享删除；禁止路径替换，不阻塞已有 SQLite 写句柄。
        options
            .access_mode(if directory { 0x80 } else { 0x80000000 })
            .share_mode(1 | 2)
            .custom_flags(0x00200000 | if directory { 0x02000000 } else { 0 });
        let file = options.open(path).context("无法固定库存源路径")?;
        let pin = Self {
            path: path.into(),
            identity: source_identity(&file, directory)?,
            file,
            directory,
        };
        pin.verify()?;
        Ok(pin)
    }

    fn verify(&self) -> Result<()> {
        ensure!(
            source_metadata(&self.path)? == self.directory,
            "库存路径类型发生变化"
        );
        ensure!(
            source_identity(&self.file, self.directory)? == self.identity,
            "库存源句柄身份发生变化"
        );
        let mut options = OpenOptions::new();
        options
            .access_mode(0x80)
            .share_mode(1 | 2)
            .custom_flags(0x00200000 | if self.directory { 0x02000000 } else { 0 });
        let current = options.open(&self.path).context("无法复核库存源路径")?;
        ensure!(
            source_identity(&current, self.directory)? == self.identity,
            "库存路径身份发生变化"
        );
        Ok(())
    }
}

fn source_metadata(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path).context("无法读取库存路径属性")?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.file_attributes() & 0x400 == 0,
        "库存拒绝链接和重解析点"
    );
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "库存拒绝非常规文件"
    );
    Ok(metadata.is_dir())
}

fn source_identity(file: &File, directory: bool) -> Result<(u32, u64)> {
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // 文件在整个调用期间持有句柄；读取属性不读取数据库内容。
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(info.dwFileAttributes & 0x400 == 0, "库存句柄拒绝重解析点");
    ensure!(
        (info.dwFileAttributes & 0x10 != 0) == directory,
        "库存句柄类型不符"
    );
    ensure!(
        directory || info.nNumberOfLinks == 1,
        "库存拒绝多硬链接文件"
    );
    Ok((
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

fn safe_component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.ends_with([' ', '.'])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "库存路径含不安全的名称"
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let numbered = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    ensure!(
        !numbered
            && !matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ),
        "库存拒绝设备名称"
    );
    Ok(())
}

/// 整次扫描持有同一库存；算法只消费内存中的 salt 和首页，不重新找文件。
struct CheckedInventory {
    directories: Vec<SourcePin>,
    files: BTreeMap<PathBuf, SourcePin>,
    salts: Vec<(String, String)>,
    pages: Vec<config_cipher::DbPage>,
    entries_left: usize,
}

impl CheckedInventory {
    fn collect(root: &Path) -> Result<Self> {
        ensure!(
            root.is_absolute() && root.as_os_str().len() <= 32760,
            "必须显式指定本机绝对数据库目录"
        );
        let raw = root.to_str().context("库存路径编码无效")?;
        ensure!(
            !raw.split(['/', '\\']).any(|s| s == "." || s == ".."),
            "库存路径不能含相对分量"
        );
        let mut inventory = Self {
            directories: Vec::new(),
            files: BTreeMap::new(),
            salts: Vec::new(),
            pages: Vec::new(),
            entries_left: MAX_SOURCE_ENTRIES,
        };
        let mut ancestor = PathBuf::new();
        for component in root.components() {
            match component {
                Component::Prefix(prefix) => {
                    let drive = match prefix.kind() {
                        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
                        _ => anyhow::bail!("库存仅允许本机磁盘路径，拒绝网络和设备路径"),
                    };
                    let drive_root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
                    let drive_type = unsafe {
                        windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(
                            drive_root.as_ptr(),
                        ))
                    };
                    ensure!(
                        matches!(drive_type, 2 | 3 | 5 | 6),
                        "库存拒绝网络映射盘或不可用磁盘"
                    );
                    ancestor.push(component);
                    continue;
                }
                Component::RootDir => {}
                Component::Normal(name) => {
                    safe_component(name.to_str().context("库存路径编码无效")?)?
                }
                _ => anyhow::bail!("库存路径分量无效"),
            }
            ancestor.push(component);
            inventory.pin_directory(&ancestor)?;
        }
        inventory.walk(root, root, 0)?;
        inventory.verify()?;
        inventory.read_pages(root)?;
        inventory.verify()?;
        Ok(inventory)
    }

    fn pin_directory(&mut self, path: &Path) -> Result<()> {
        ensure!(
            self.directories.len() < MAX_SOURCE_DIRECTORIES,
            "库存目录数量超限"
        );
        self.directories.push(SourcePin::open(path, true)?);
        Ok(())
    }

    fn verify(&self) -> Result<()> {
        for pin in self.directories.iter().chain(self.files.values()) {
            pin.verify()?;
        }
        Ok(())
    }

    fn walk(&mut self, root: &Path, directory: &Path, depth: usize) -> Result<()> {
        ensure!(depth <= MAX_SOURCE_DEPTH, "库存目录深度超限");
        for pin in self
            .directories
            .iter()
            .filter(|pin| directory.starts_with(&pin.path))
        {
            pin.verify()?;
        }
        for entry in fs::read_dir(directory).context("无法枚举库存目录")? {
            self.entries_left = self
                .entries_left
                .checked_sub(1)
                .context("库存条目数量超限")?;
            let entry = entry.context("无法读取库存目录条目")?;
            let path = entry.path();
            safe_component(entry.file_name().to_str().context("库存文件名编码无效")?)?;
            let is_directory = source_metadata(&path)?;
            if super::is_migration_path(root, &path) {
                continue;
            }
            if is_directory {
                self.pin_directory(&path)?;
                self.walk(root, &path, depth + 1)?;
            } else if path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("db") || s.eq_ignore_ascii_case("db-wal"))
            {
                ensure!(self.files.len() < MAX_SOURCE_FILES, "库存文件数量超限");
                let pin = SourcePin::open(&path, false)?;
                self.files.insert(path, pin);
            }
        }
        Ok(())
    }

    fn read_pages(&mut self, root: &Path) -> Result<()> {
        let mut bytes_left = MAX_SOURCE_BYTES;
        let mut frames_left = MAX_WAL_FRAMES;
        let mut database_count = 0;
        for (path, pin) in &self.files {
            if !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("db"))
            {
                continue;
            }
            database_count += 1;
            ensure!(database_count <= MAX_DATABASES, "库存数据库数量超限");
            pin.verify()?;
            let mut file = pin.file.try_clone()?;
            let mut page = vec![0u8; crate::crypto::PAGE_SZ];
            config_cipher::read_budgeted(&mut file, &mut page[..16], &mut bytes_left)?;
            if &page[..15] == b"SQLite format 3" {
                continue;
            }
            config_cipher::read_budgeted(&mut file, &mut page[16..], &mut bytes_left)?;
            pin.verify()?;
            let name = path
                .strip_prefix(root)
                .context("库存文件越过账号根目录")?
                .to_str()
                .context("库存相对路径编码无效")?
                .replace('\\', "/");
            let salt = super::hex::encode(&page[..16]);
            let mut variants = Vec::with_capacity(2);
            // 只使用枚举时已固定的 WAL；不追踪随后出现的侧文件。
            let wal_path = path.with_extension("db-wal");
            if let Some((_, wal)) = self.files.iter().find(|(candidate, _)| {
                candidate
                    .as_os_str()
                    .eq_ignore_ascii_case(wal_path.as_os_str())
            }) {
                wal.verify()?;
                let mut file = wal.file.try_clone()?;
                if let Some(page) = config_cipher::read_wal_page1_from(
                    &mut file,
                    &mut frames_left,
                    &mut bytes_left,
                )? {
                    variants.push(page);
                }
                wal.verify()?;
            }
            variants.push(page);
            self.salts.push((salt, name.clone()));
            self.pages.push(config_cipher::DbPage {
                db_name: name,
                page1_variants: variants,
            });
        }
        Ok(())
    }
}

pub fn scan_keys(db_dir: &Path, process_name: &str) -> Result<Vec<KeyEntry>> {
    safe_component(process_name)?;
    ensure!(
        process_name.to_ascii_lowercase().ends_with(".exe"),
        "必须显式指定进程的 .exe 文件名"
    );
    let inventory = CheckedInventory::collect(db_dir)?;
    let db_salts = &inventory.salts;
    ensure!(!db_salts.is_empty(), "指定目录没有可扫描的加密数据库");
    let targets: BTreeSet<String> = db_salts.iter().map(|(_, name)| name.clone()).collect();
    eprintln!("找到 {} 个加密数据库", db_salts.len());

    let pids = find_wechat_pids(process_name);
    if pids.is_empty() {
        anyhow::bail!("找不到 {} 进程，请确认微信正在运行", process_name);
    }

    let (wechat_version, image_path) = detect_version(&pids)?;
    eprintln!(
        "检测到微信版本: {} ({})",
        wechat_version,
        image_path.display()
    );
    let config_cipher_version = version::WechatVersion {
        major: 4,
        minor: 1,
        build: 10,
        revision: 0,
    };
    if wechat_version < config_cipher_version {
        eprintln!(
            "按微信版本 {} 选择 legacy raw-key provider...",
            wechat_version
        );
        return scan_legacy_pids(
            &pids,
            &targets,
            || inventory.verify(),
            |pid| {
                let process = open_process(pid)?;
                let found = legacy::scan(process, &inventory.pages);
                unsafe {
                    let _ = CloseHandle(process);
                }
                found
            },
        );
    }

    let mut entries = Vec::new();
    eprintln!(
        "按微信版本 {} 选择 Config.Cipher 只读扫描（{} 个进程）...",
        wechat_version,
        pids.len()
    );
    for pid in &pids {
        inventory.verify()?;
        let Ok(process) = open_process(*pid) else {
            eprintln!("跳过 PID {}：无法打开进程", pid);
            continue;
        };
        let result = config_cipher::scan(process, &inventory.pages);
        unsafe {
            let _ = CloseHandle(process);
        }
        inventory.verify()?;
        match result {
            Ok(found) => merge_entries(
                &mut entries,
                found
                    .into_iter()
                    .filter(|entry| targets.contains(&entry.db_name))
                    .collect(),
            ),
            Err(error) => eprintln!("Config.Cipher 扫描 PID {} 失败: {error:#}", pid),
        }
        if missing_databases(&targets, &entries).is_empty() {
            break;
        }
    }
    let missing = missing_databases(&targets, &entries);
    if missing.is_empty() {
        eprintln!(
            "Config.Cipher 已完整验证 {}/{} 个数据库",
            entries.len(),
            targets.len()
        );
        return Ok(entries);
    }
    anyhow::bail!(
        "Config.Cipher 未完整验证数据库密钥：{}/{}；缺失：{}；未执行旧版回退，现有密钥文件未修改",
        targets.len() - missing.len(),
        targets.len(),
        missing.join(", ")
    )
}

fn missing_databases(targets: &BTreeSet<String>, entries: &[KeyEntry]) -> Vec<String> {
    let verified: BTreeSet<&str> = entries.iter().map(|entry| entry.db_name.as_str()).collect();
    targets
        .iter()
        .filter(|name| !verified.contains(name.as_str()))
        .cloned()
        .collect()
}

/// 单个进程不可访问时继续；库存身份错误直接终止，不降级为进程失败。
fn scan_legacy_pids(
    pids: &[u32],
    targets: &BTreeSet<String>,
    mut verify_source: impl FnMut() -> Result<()>,
    mut scan_pid: impl FnMut(u32) -> Result<Vec<KeyEntry>>,
) -> Result<Vec<KeyEntry>> {
    let mut entries = Vec::new();
    let mut failed_pids = Vec::new();
    for &pid in pids {
        verify_source()?;
        let result = scan_pid(pid);
        verify_source()?;
        match result {
            Ok(found) => merge_entries(
                &mut entries,
                found
                    .into_iter()
                    .filter(|entry| targets.contains(&entry.db_name))
                    .collect(),
            ),
            Err(_) => {
                // 不回显底层候选内容；最终错误只给出失败进程和缺失库名。
                failed_pids.push(pid);
                eprintln!("legacy 跳过 PID {pid}：无法打开或完成扫描");
            }
        }
        if missing_databases(targets, &entries).is_empty() {
            eprintln!(
                "legacy 已完整验证 {}/{} 个数据库",
                entries.len(),
                targets.len()
            );
            return Ok(entries);
        }
    }
    let missing = missing_databases(targets, &entries);
    anyhow::bail!(
        "legacy 未完整验证数据库密钥：{}/{}；缺失：{}；无法扫描的 PID：{:?}；密钥文件未修改",
        targets.len() - missing.len(),
        targets.len(),
        missing.join(", "),
        failed_pids
    )
}

fn detect_version(pids: &[u32]) -> Result<(version::WechatVersion, std::path::PathBuf)> {
    for pid in pids {
        let Ok(process) = open_process(*pid) else {
            continue;
        };
        let result = version::detect(process);
        unsafe {
            let _ = CloseHandle(process);
        }
        if let Ok(version) = result {
            return Ok(version);
        }
    }
    anyhow::bail!("无法读取 Weixin.exe 文件版本，已停止扫描")
}

fn find_wechat_pids(process_name: &str) -> Vec<u32> {
    let Some(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok() }) else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    let mut entry = PROCESSENTRY32 {
        dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
        ..Default::default()
    };
    unsafe {
        if Process32First(snap, &mut entry).is_ok() {
            loop {
                let name = std::ffi::CStr::from_ptr(entry.szExeFile.as_ptr()).to_string_lossy();
                if name.eq_ignore_ascii_case(process_name) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32Next(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

fn open_process(pid: u32) -> Result<HANDLE> {
    unsafe {
        OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
            .context("OpenProcess 失败，请以管理员权限运行")
    }
}

fn merge_entries(target: &mut Vec<KeyEntry>, found: Vec<KeyEntry>) {
    for entry in found {
        if !target
            .iter()
            .any(|existing| existing.db_name == entry.db_name)
        {
            target.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> KeyEntry {
        KeyEntry {
            db_name: name.into(),
            enc_key: String::new(),
            salt: String::new(),
        }
    }

    #[test]
    fn extra_and_duplicate_keys_cannot_hide_missing_database() {
        let targets = BTreeSet::from(["message/a.db".into(), "message/b.db".into()]);
        let entries = vec![
            entry("message/a.db"),
            entry("message/a.db"),
            entry("migrate/old.db"),
        ];
        assert_eq!(missing_databases(&targets, &entries), vec!["message/b.db"]);
    }

    #[test]
    fn complete_keys_can_be_merged_across_processes() {
        let targets = BTreeSet::from(["message/a.db".into(), "message/b.db".into()]);
        let mut entries = vec![entry("message/b.db")];
        merge_entries(
            &mut entries,
            vec![entry("message/a.db"), entry("message/b.db")],
        );
        assert!(missing_databases(&targets, &entries).is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn legacy_second_pid_completes_inventory_and_deduplicates() {
        let targets = BTreeSet::from(["message/a.db".into(), "message/b.db".into()]);
        let mut visited = Vec::new();
        let entries = scan_legacy_pids(
            &[10, 20, 30],
            &targets,
            || Ok(()),
            |pid| {
                visited.push(pid);
                Ok(if pid == 10 {
                    vec![entry("message/a.db")]
                } else {
                    vec![
                        entry("message/a.db"),
                        entry("message/b.db"),
                        entry("outside.db"),
                    ]
                })
            },
        )
        .unwrap();
        assert_eq!(visited, [10, 20]);
        assert_eq!(entries.len(), 2);
        assert!(missing_databases(&targets, &entries).is_empty());
    }

    #[test]
    fn legacy_inaccessible_pid_does_not_prevent_later_success() {
        let targets = BTreeSet::from(["a.db".into()]);
        let mut visited = Vec::new();
        let entries = scan_legacy_pids(
            &[10, 20],
            &targets,
            || Ok(()),
            |pid| {
                visited.push(pid);
                if pid == 10 {
                    anyhow::bail!("不可访问的模拟进程");
                }
                Ok(vec![entry("a.db")])
            },
        )
        .unwrap();
        assert_eq!(visited, [10, 20]);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn legacy_incomplete_result_reports_missing_and_failed_pid_without_secrets() {
        let targets = BTreeSet::from(["a.db".into(), "b.db".into()]);
        let error = scan_legacy_pids(
            &[10, 20],
            &targets,
            || Ok(()),
            |pid| {
                if pid == 10 {
                    anyhow::bail!("synthetic-secret-must-not-escape");
                }
                Ok(vec![entry("a.db")])
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("1/2"));
        assert!(error.contains("b.db"));
        assert!(error.contains("10"));
        assert!(!error.contains("synthetic-secret"));
    }

    #[test]
    fn legacy_source_failure_is_not_swallowed_as_a_pid_failure() {
        let targets = BTreeSet::from(["a.db".into()]);
        let mut called = false;
        let result = scan_legacy_pids(
            &[10, 20],
            &targets,
            || anyhow::bail!("模拟库存身份变化"),
            |_| {
                called = true;
                Ok(vec![entry("a.db")])
            },
        );
        assert!(result.is_err());
        assert!(!called);
    }
}
