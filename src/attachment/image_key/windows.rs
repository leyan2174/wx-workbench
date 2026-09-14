//! 固定账号的 Windows V2 图片密钥提取；不写配置、不输出密钥、不枚举账号。
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, HANDLE,
};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_NOCACHE, PAGE_READWRITE,
    PAGE_WRITECOMBINE, PAGE_WRITECOPY,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_INFORMATION,
    PROCESS_VM_READ,
};

use super::{
    ascii_alnum_matches, derive_xor_bounded, find_templates_bounded, same_wxid, verify_aes_key,
    ExtractionBudget, ImageKeyMaterial, ImageKeyProvider, NoImageKeyFound, NoImageKeyReason,
};

const CHUNK_SIZE: usize = 2 * 1024 * 1024;
const OVERLAP: usize = 33;
const MAX_PROCESSES: usize = 128;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub struct WindowsImageKeyProvider {
    configured: std::result::Result<(PathBuf, String), String>,
    timeout: Duration,
    max_bytes: u64,
    cache: Mutex<Option<ImageKeyMaterial>>,
}

impl Drop for WindowsImageKeyProvider {
    fn drop(&mut self) {
        let cached = match self.cache.get_mut() {
            Ok(value) => value,
            Err(error) => error.into_inner(),
        };
        if let Some(material) = cached.as_mut() {
            material.aes_key.zeroize();
            material.xor_key.zeroize();
        }
    }
}

impl WindowsImageKeyProvider {
    pub fn from_current_config() -> Self {
        Self {
            configured: crate::config::load_config()
                .map(|cfg| (cfg.db_dir, cfg.wechat_process))
                .map_err(|err| err.to_string()),
            timeout: DEFAULT_TIMEOUT,
            max_bytes: DEFAULT_MAX_BYTES,
            cache: Mutex::new(None),
        }
    }

    /// 只固定显式参数；构造不读取配置、目录或进程。get_key 才执行有界提取。
    pub fn from_db_dir(
        db_dir: &Path,
        process_name: &str,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Self> {
        validate_inputs(db_dir, process_name)?;
        ExtractionBudget::new(timeout, max_bytes)?;
        Ok(Self {
            configured: Ok((db_dir.into(), process_name.into())),
            timeout,
            max_bytes,
            cache: Mutex::new(None),
        })
    }
}

impl ImageKeyProvider for WindowsImageKeyProvider {
    fn get_key(&self, wxid: &str) -> Result<ImageKeyMaterial> {
        let (db_dir, process_name) = self
            .configured
            .as_ref()
            .map_err(|err| anyhow::anyhow!("读取图片提取配置失败：{err}"))?;
        let account = db_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .context("固定 db_dir 缺少账号目录名")?;
        // 保留原有 wxid 规范化匹配，但不再据此改写 db_dir 或发现兄弟账号。
        ensure!(
            wxid.trim().is_empty() || same_wxid(account, wxid),
            "请求 wxid 与固定账号不符，拒绝跨账号查找"
        );
        let cached = *self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("图片密钥缓存锁异常"))?;
        if let Some(key) = cached {
            return Ok(key);
        }
        let key = extract_for_db_dir(db_dir, process_name, self.timeout, self.max_bytes)?;
        *self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("图片密钥缓存锁异常"))? = Some(key);
        Ok(key)
    }
}

fn validate_inputs(db_dir: &Path, process_name: &str) -> Result<()> {
    ensure!(
        db_dir.is_absolute() && db_dir.parent().is_some(),
        "db_dir 必须为显式本机绝对目录"
    );
    let raw = db_dir.to_str().context("db_dir 编码无效")?;
    ensure!(
        raw.len() <= 32760 && !raw.split(['/', '\\']).any(|s| s == "." || s == ".."),
        "db_dir 含不安全路径组件"
    );
    for component in db_dir.components() {
        match component {
            std::path::Component::Normal(name) => {
                crate::attachment::local_files::safe_name(name.to_str().context("路径编码无效")?)?
            }
            std::path::Component::Prefix(prefix) => ensure!(
                matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ),
                "仅支持本机磁盘路径"
            ),
            std::path::Component::RootDir => {}
            _ => bail!("db_dir 含相对路径组件"),
        }
    }
    ensure!(
        process_name.len() <= 260
            && process_name == process_name.trim()
            && process_name.to_ascii_lowercase().ends_with(".exe"),
        "process_name 必须为普通 exe 文件名"
    );
    crate::attachment::local_files::safe_name(process_name)?;
    Ok(())
}

/// 同一调用的截止时间、读取字节与候选预算覆盖目录采样及全部匹配 PID。
/// max_bytes 计入文件头/尾与进程读取请求（失败读取也计费），不是每个进程分别重置。
/// 截止时间在系统调用前后检查；不承诺强制取消已进入内核的同步调用。
/// 枚举全部同名进程，逐个尝试，只有通过固定账号全部模板反验的候选才可提前返回。
pub fn extract_for_db_dir(
    db_dir: &Path,
    process_name: &str,
    timeout: Duration,
    max_bytes: u64,
) -> Result<ImageKeyMaterial> {
    let mut budget = ExtractionBudget::new(timeout, max_bytes)?;
    validate_inputs(db_dir, process_name)?;
    let mut source = crate::attachment::local_files::Scan::new();
    source.root(db_dir)?;
    let attach_dir = db_dir
        .parent()
        .context("db_dir 缺少父目录")?
        .join("msg/attach");
    let templates = find_templates_bounded(&attach_dir, 3, 4096, &mut budget)?;
    ensure!(!templates.is_empty(), "固定账号附件目录没有可用的 V2 模板");
    // 沿用原 XOR 核心语义：样本不足时使用协议默认值，不能把预算或 I/O 错误当作样本不足。
    let xor_key = derive_xor_bounded(&attach_dir, 10, 3, &mut budget)?
        .map(|(key, _, _)| key)
        .unwrap_or(0x88);
    let pids = find_matching_pids(process_name, &mut budget)?;
    let mut seen = HashSet::<[u8; 32]>::new();
    let mut failures = Vec::new();
    for pid in pids {
        if let Err(error) = budget.check() {
            ensure!(
                failures.is_empty(),
                "本轮存在进程错误，不能作为正常轮次到期重试：{}",
                failures.join("；")
            );
            return Err(error);
        }
        let result = (|| -> Result<Option<[u8; 16]>> {
            let process = OwnedHandle(
                unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid) }
                    .context("无法以只读权限打开匹配进程")?,
            );
            verify_process_name(process.0, process_name, &budget)?;
            scan_memory_for_key(process.0, &templates, &mut seen, &mut budget)
        })();
        match result {
            Ok(Some(aes_key)) => {
                source.verify()?;
                budget.check()?;
                return Ok(ImageKeyMaterial { aes_key, xor_key });
            }
            Ok(None) => {}
            Err(error) => {
                if error.is::<NoImageKeyFound>() {
                    ensure!(
                        failures.is_empty(),
                        "本轮扫描同时存在进程错误，不能作为无命中重试：{}",
                        failures.join("；")
                    );
                    return Err(error);
                }
                failures.push(format!("PID {pid}: {error:#}"));
            }
        }
    }
    source.verify()?;
    if !failures.is_empty() {
        bail!(
            "未找到通过账号模板验证的密钥；以下进程扫描未完成：{}",
            failures.join("；")
        );
    }
    budget.check()?;
    Err(NoImageKeyFound {
        reason: NoImageKeyReason::CompletedScan,
    }
    .into())
}

/// 只读验证配置中已有 AES-128 密钥；不读取或枚举进程，不读取全局配置，不写验证图片。
/// 无模板及路径/权限/预算错误保留为 Err，只有完成模板反验但不匹配时返回 false。
pub fn validate_existing_for_db_dir(
    db_dir: &Path,
    aes_key: &[u8; 16],
    timeout: Duration,
    max_bytes: u64,
) -> Result<bool> {
    let mut budget = ExtractionBudget::new(timeout, max_bytes)?;
    // 复用纯参数校验；此处的文件名不用于任何进程访问。
    validate_inputs(db_dir, "Weixin.exe")?;
    let mut source = crate::attachment::local_files::Scan::new();
    source.root(db_dir)?;
    let attach_dir = db_dir
        .parent()
        .context("db_dir 缺少父目录")?
        .join("msg/attach");
    let templates = find_templates_bounded(&attach_dir, 3, 4096, &mut budget)?;
    ensure!(
        !templates.is_empty(),
        "固定账号附件目录没有可用的 V2 模板，无法验证现有密钥"
    );
    budget.check()?;
    let valid = verify_aes_key(aes_key, &templates);
    source.verify()?;
    budget.check()?;
    Ok(valid)
}

fn no_more_files(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0)
}

fn find_matching_pids(process_name: &str, budget: &mut ExtractionBudget) -> Result<Vec<u32>> {
    budget.check()?;
    let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    match unsafe { Process32FirstW(snapshot.0, &mut entry) } {
        Ok(()) => {}
        Err(error) if no_more_files(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    }
    let mut pids = Vec::new();
    loop {
        budget.entry()?;
        let length = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .context("进程快照文件名未终止")?;
        let name =
            String::from_utf16(&entry.szExeFile[..length]).context("进程快照文件名编码无效")?;
        if name.eq_ignore_ascii_case(process_name) {
            ensure!(
                pids.len() < MAX_PROCESSES,
                "匹配进程数量超过 128，拒绝截断扫描"
            );
            pids.push(entry.th32ProcessID);
        }
        budget.check()?;
        match unsafe { Process32NextW(snapshot.0, &mut entry) } {
            Ok(()) => {}
            Err(error) if no_more_files(&error) => break,
            Err(error) => return Err(error.into()),
        }
    }
    pids.sort_unstable();
    pids.dedup();
    budget.check()?;
    Ok(pids)
}

fn verify_process_name(process: HANDLE, expected: &str, budget: &ExtractionBudget) -> Result<()> {
    budget.check()?;
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(path.as_mut_ptr()),
            &mut length,
        )
    }?;
    budget.check()?;
    ensure!((length as usize) <= path.len(), "进程映像路径长度无效");
    let path = PathBuf::from(String::from_utf16(&path[..length as usize])?);
    ensure!(
        path.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(expected)),
        "进程身份已变化，拒绝扫描复用的 PID"
    );
    Ok(())
}

fn scan_memory_for_key(
    process: HANDLE,
    templates: &[[u8; 16]],
    seen: &mut HashSet<[u8; 32]>,
    budget: &mut ExtractionBudget,
) -> Result<Option<[u8; 16]>> {
    let mut address = 0usize;
    loop {
        budget.region()?;
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let returned = unsafe {
            VirtualQueryEx(
                process,
                Some(address as *const _),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        let error = if returned == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };
        budget.check()?;
        if let Some(error) = error {
            if error == ERROR_INVALID_PARAMETER {
                break;
            }
            bail!("VirtualQueryEx 失败，Windows 错误码 {}", error.0);
        }
        ensure!(
            returned == std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            "VirtualQueryEx 返回结构不完整"
        );
        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        let next = base.checked_add(size).context("进程内存区域地址溢出")?;
        ensure!(
            size > 0 && base <= address && next > address,
            "进程内存枚举未向前推进"
        );
        // 不再按区域总大小静默跳过；大区域仍以固定尺寸块读取并受同一预算限制。
        if info.State == MEM_COMMIT && is_candidate_page(info.Protect.0) {
            if let Some(key) = scan_region(process, base, size, templates, seen, budget)? {
                return Ok(Some(key));
            }
        }
        address = next;
    }
    Ok(None)
}

fn scan_region(
    process: HANDLE,
    base: usize,
    size: usize,
    templates: &[[u8; 16]],
    seen: &mut HashSet<[u8; 32]>,
    budget: &mut ExtractionBudget,
) -> Result<Option<[u8; 16]>> {
    let mut offset = 0usize;
    while offset < size {
        budget.check()?;
        if budget.remaining_bytes() == 0 {
            return Err(NoImageKeyFound {
                reason: NoImageKeyReason::ByteBudget,
            }
            .into());
        }
        let chunk = CHUNK_SIZE
            .min(size - offset)
            .min(budget.remaining_bytes().min(usize::MAX as u64) as usize);
        budget.read(chunk)?;
        let mut buf = Zeroizing::new(vec![0u8; chunk]);
        let mut bytes_read = 0usize;
        let address = base.checked_add(offset).context("内存块地址溢出")?;
        let result = unsafe {
            ReadProcessMemory(
                process,
                address as *const _,
                buf.as_mut_ptr() as *mut _,
                chunk,
                Some(&mut bytes_read),
            )
        };
        // 操作系统错误优先于轮次到期，不能把权限或部分读取失败包装成可重试的无命中。
        result.context("进程内存读取失败，本进程扫描未完成")?;
        budget.check()?;
        ensure!(bytes_read <= chunk, "ReadProcessMemory 返回长度无效");
        ensure!(bytes_read == chunk, "进程内存读取不完整，本进程扫描未完成");
        if bytes_read > 0 {
            // 重叠包含完整候选和两侧边界；块边缘不能冒充字符串边界。
            if let Some(key) = scan_candidate_buffer(
                &buf[..bytes_read],
                templates,
                seen,
                budget,
                offset == 0,
                offset + bytes_read == size,
            )? {
                return Ok(Some(key));
            }
        }
        // buf 不 truncate，离开作用域时清零整个分配区域（包括失败/部分读取路径）。
        let step = if chunk == size - offset {
            chunk
        } else if chunk > OVERLAP {
            chunk - OVERLAP
        } else {
            chunk
        };
        offset = offset.checked_add(step).context("内存块偏移溢出")?;
    }
    Ok(None)
}

fn scan_candidate_buffer(
    buf: &[u8],
    templates: &[[u8; 16]],
    seen: &mut HashSet<[u8; 32]>,
    budget: &mut ExtractionBudget,
    left_boundary: bool,
    right_boundary: bool,
) -> Result<Option<[u8; 16]>> {
    for length in [32, 16] {
        for range in ascii_alnum_matches(buf, length) {
            budget.candidate()?;
            if (range.start == 0 && !left_boundary) || (range.end == buf.len() && !right_boundary) {
                continue;
            }
            let candidate = &buf[range];
            let mut key = Zeroizing::new([0u8; 16]);
            key.copy_from_slice(&candidate[..16]);
            // 去重集只持有摘要，避免批量长期保留进程内找到的明文候选。
            let fingerprint: [u8; 32] = Sha256::digest(*key).into();
            if seen.insert(fingerprint) && verify_aes_key(&key, templates) {
                budget.check()?;
                return Ok(Some(*key));
            }
        }
    }
    budget.check()?;
    Ok(None)
}

fn is_candidate_page(protect: u32) -> bool {
    if protect == PAGE_NOACCESS.0 || protect & PAGE_GUARD.0 != 0 {
        return false;
    }
    let base = protect & !(PAGE_GUARD.0 | PAGE_NOCACHE.0 | PAGE_WRITECOMBINE.0);
    matches!(base,value if value==PAGE_READWRITE.0 || value==PAGE_WRITECOPY.0 || value==PAGE_EXECUTE_READWRITE.0 || value==PAGE_EXECUTE_WRITECOPY.0)
}
