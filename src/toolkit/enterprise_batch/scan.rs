//! 企业微信候选识别，与通用进程枚举/读取解耦；绝不输出原始内存、地址或密钥。
use super::{failure, inventory, Account, Database, Failure, KeyRing};
use anyhow::{ensure, Result};
use regex::bytes::Regex;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
}

/// main 可用 scanner 通用 Windows 接口实现此适配器；只允许只读进程句柄。
pub trait ProcessMemory {
    fn pointer_width(&self) -> Result<u8>;
    fn regions(&self) -> Result<Vec<MemoryRegion>>;
    fn read(&self, address: u64, buffer: &mut [u8]) -> Result<usize>;
}

pub trait ProcessScanner {
    /// 只返回映像名称恰为 WXWork.exe 的进程，不接收任意进程名。
    fn wxwork_pids(&self) -> Result<Vec<u32>>;
    fn open(&self, pid: u32) -> Result<Box<dyn ProcessMemory>>;
}

#[derive(Clone)]
pub struct ScanOptions {
    pub authorized: bool,
    pub pids: Vec<u32>,
    pub bare_hex: bool,
    pub cipher_structs: bool,
    /// 所有进程共用墙钟预算，不是每个进程重新计时。
    pub max_seconds: u64,
    /// 包括重叠块和指针追踪读取在内的累计读取预算。
    pub max_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            authorized: false,
            pids: Vec::new(),
            bare_hex: false,
            cipher_structs: true,
            max_seconds: 120,
            max_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}
impl ScanOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.authorized,
            "进程取钥须显式授权 --authorize-memory-scan"
        );
        ensure!(
            self.max_seconds > 0 && self.max_bytes >= 4096,
            "扫描时间必须大于零且字节预算至少 4096"
        );
        ensure!(self.pids.iter().all(|pid| *pid != 0), "无效进程 ID");
        Ok(())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    pub account_id: String,
    pub encrypted_databases: usize,
    pub verified_databases: usize,
    pub missing_databases: Vec<String>,
    pub processes_seen: usize,
    pub processes_opened: usize,
    pub bytes_read: u64,
    pub bytes_requested: u64,
    pub read_failures: usize,
    pub candidates_checked: usize,
    pub structure_64bit_skipped: usize,
    pub budget_exhausted: bool,
    pub elapsed_millis: u128,
    pub failed: usize,
    pub failures: Vec<Failure>,
}

/// 返回不透明的账号绑定 KeyRing；调用方可复用解密或显式私有文件发布，报告不含密钥。
pub fn scan_authorized(
    source: &Path,
    options: &ScanOptions,
    scanner: &dyn ProcessScanner,
) -> Result<(KeyRing, ScanReport)> {
    options.validate()?;
    let account = Account::open(source)?;
    let (databases, failures) = inventory(&account)?;
    let mut keys = KeyRing::new(&account);
    let mut report = fill(&account, &databases, &mut keys, options, scanner)?;
    report.failures.extend(failures);
    report.failed = report.missing_databases.len() + report.failures.len();
    Ok((keys, report))
}

struct Budget {
    start: Instant,
    duration: Duration,
    limit: u64,
}
impl Budget {
    fn timed_out(&self) -> bool {
        self.start.elapsed() >= self.duration
    }
    fn exhausted(&self, report: &ScanReport) -> bool {
        self.start.elapsed() >= self.duration || report.bytes_requested >= self.limit
    }
    fn read(
        &self,
        memory: &dyn ProcessMemory,
        address: u64,
        output: &mut [u8],
        report: &mut ScanReport,
    ) -> bool {
        if self.exhausted(report)
            || output.len() as u64 > self.limit.saturating_sub(report.bytes_requested)
        {
            report.budget_exhausted = true;
            return false;
        }
        // 请求字节也计入预算，避免无法读取的区域造成无界重试。
        report.bytes_requested += output.len() as u64;
        match memory.read(address, output) {
            Ok(size) if size <= output.len() => {
                report.bytes_read += size as u64;
                if size == output.len() {
                    true
                } else {
                    report.read_failures += 1;
                    false
                }
            }
            _ => {
                report.read_failures += 1;
                false
            }
        }
    }
}

fn complete(databases: &[Database], keys: &KeyRing) -> bool {
    databases
        .iter()
        .all(|database| database.plain || keys.contains(&database.relative))
}

pub(crate) fn fill(
    account: &Account,
    databases: &[Database],
    keys: &mut KeyRing,
    options: &ScanOptions,
    scanner: &dyn ProcessScanner,
) -> Result<ScanReport> {
    options.validate()?;
    let budget = Budget {
        start: Instant::now(),
        duration: Duration::from_secs(options.max_seconds),
        limit: options.max_bytes,
    };
    let mut report = ScanReport {
        account_id: account.account_id.clone(),
        encrypted_databases: databases.iter().filter(|d| !d.plain).count(),
        ..Default::default()
    };
    if !complete(databases, keys) {
        let detected = match scanner.wxwork_pids() {
            Ok(pids) => pids.into_iter().collect::<BTreeSet<_>>(),
            Err(_) => {
                report.failures.push(failure(
                    "WXWork.exe".into(),
                    "scan",
                    "process_enumeration_failed",
                ));
                BTreeSet::new()
            }
        };
        report.processes_seen = detected.len();
        let selected = if options.pids.is_empty() {
            detected.clone()
        } else {
            let requested: BTreeSet<_> = options.pids.iter().copied().collect();
            ensure!(
                requested.is_subset(&detected),
                "指定 PID 不属于当前 WXWork.exe 进程；未读取内存"
            );
            requested
        };
        if selected.is_empty() {
            report
                .failures
                .push(failure("WXWork.exe".into(), "scan", "process_not_running"));
        }
        let literal = Regex::new(r"x'([0-9a-fA-F]{32,192})'")?;
        let bare = Regex::new(r"[0-9a-fA-F]{32,}")?;
        for pid in selected {
            if complete(databases, keys) {
                break;
            }
            if budget.exhausted(&report) {
                report.budget_exhausted = true;
                break;
            }
            let memory = match scanner.open(pid) {
                Ok(memory) => memory,
                Err(_) => {
                    report
                        .failures
                        .push(failure(pid.to_string(), "scan", "process_open_failed"));
                    continue;
                }
            };
            report.processes_opened += 1;
            let width = memory.pointer_width().unwrap_or(0);
            if options.cipher_structs && width != 4 {
                report.structure_64bit_skipped += 1;
                report.failures.push(failure(
                    pid.to_string(),
                    "scan",
                    if width == 8 {
                        "cipher_layout_64bit_unsupported"
                    } else {
                        "process_bitness_unknown"
                    },
                ));
            }
            let mut regions = match memory.regions() {
                Ok(regions) => regions,
                Err(_) => {
                    report.failures.push(failure(
                        pid.to_string(),
                        "scan",
                        "memory_regions_unavailable",
                    ));
                    continue;
                }
            };
            regions.sort_by_key(|r| r.base);
            for region in regions {
                if complete(databases, keys) {
                    break;
                }
                let Some(end) = region.base.checked_add(region.size) else {
                    continue;
                };
                let mut address = region.base;
                while address < end {
                    if complete(databases, keys) {
                        break;
                    }
                    if budget.exhausted(&report) || report.budget_exhausted {
                        report.budget_exhausted = true;
                        break;
                    }
                    // 块边界保留 256 字节重叠，覆盖最长 SQL 字面量及 0x40 字节结构体。
                    let length = (end - address)
                        .min(1024 * 1024)
                        .min(budget.limit.saturating_sub(report.bytes_requested))
                        as usize;
                    let mut buffer = Zeroizing::new(vec![0; length]);
                    if budget.read(&*memory, address, &mut buffer, &mut report) {
                        for capture in literal.captures_iter(&buffer) {
                            if budget.timed_out() || complete(databases, keys) {
                                break;
                            }
                            let value = capture.get(1).expect("固定正则捕获组").as_bytes();
                            if value.len() % 2 == 0 {
                                // 核心只支持 AES-128；96/更长缓存仅尝试其中的 16 字节候选。
                                offer_hex(&value[..32], databases, keys, &mut report);
                            }
                        }
                        if options.bare_hex && !complete(databases, keys) {
                            for found in bare.find_iter(&buffer) {
                                if budget.timed_out() || complete(databases, keys) {
                                    break;
                                }
                                if found.len() == 32 {
                                    offer_hex(found.as_bytes(), databases, keys, &mut report);
                                }
                            }
                        }
                        if options.cipher_structs && width == 4 && !complete(databases, keys) {
                            scan_structures(
                                &*memory,
                                address,
                                &buffer,
                                databases,
                                keys,
                                &budget,
                                &mut report,
                            );
                        }
                    }
                    if length <= 256 {
                        break;
                    }
                    address += (length - 256) as u64;
                }
                if report.budget_exhausted {
                    break;
                }
            }
        }
    }
    report.missing_databases = databases
        .iter()
        .filter(|d| !d.plain && !keys.contains(&d.relative))
        .map(|d| d.relative.clone())
        .collect();
    report.verified_databases = report.encrypted_databases - report.missing_databases.len();
    report.budget_exhausted |= budget.exhausted(&report) && !report.missing_databases.is_empty();
    report.elapsed_millis = budget.start.elapsed().as_millis();
    report.failed = report.missing_databases.len() + report.failures.len();
    Ok(report)
}

fn offer_hex(value: &[u8], databases: &[Database], keys: &mut KeyRing, report: &mut ScanReport) {
    let Ok(text) = std::str::from_utf8(value) else {
        return;
    };
    let Ok(key) = crate::toolkit::enterprise::parse_key_hex(text).map(Zeroizing::new) else {
        return;
    };
    report.candidates_checked += 1;
    // 每个候选直接验证本账号全部未匹配主库，不以相同 salt 推断密钥通用性。
    keys.offer(databases, &key);
}

fn word(buffer: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buffer[at..at + 4].try_into().expect("已检查结构边界"))
}
fn read_word(
    memory: &dyn ProcessMemory,
    address: u64,
    budget: &Budget,
    report: &mut ScanReport,
) -> Option<u32> {
    if address == 0 || address.checked_add(4)? > u32::MAX as u64 + 1 {
        return None;
    }
    let mut buffer = Zeroizing::new([0; 4]);
    if budget.read(memory, address, &mut *buffer, report) {
        Some(word(&*buffer, 0))
    } else {
        None
    }
}

fn scan_structures(
    memory: &dyn ProcessMemory,
    base: u64,
    buffer: &[u8],
    databases: &[Database],
    keys: &mut KeyRing,
    budget: &Budget,
    report: &mut ScanReport,
) {
    let mut offset = ((4 - base % 4) % 4) as usize;
    while offset + 0x40 <= buffer.len() {
        if budget.exhausted(report) || report.budget_exhausted || complete(databases, keys) {
            break;
        }
        if matches!(word(buffer, offset), 1 | 2)
            && matches!(word(buffer, offset + 4), 1 | 2 | 4096 | 8192 | 16384)
        {
            let aes_context = u64::from(word(buffer, offset + 0x2c));
            let holder = u64::from(word(buffer, offset + 0x30));
            let mut context = Zeroizing::new([0u8; 0x40]);
            if aes_context != 0
                && holder != 0
                && budget.read(memory, aes_context, &mut *context, report)
            {
                if let Some(object) = read_word(memory, holder + 4, budget, report) {
                    if object != 0
                        && matches!(
                            read_word(memory, u64::from(object) + 0x24, budget, report),
                            Some(512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768 | 65536)
                        )
                    {
                        let mut key = Zeroizing::new([0u8; 16]);
                        key.copy_from_slice(&buffer[offset + 8..offset + 24]);
                        // 只对已验证布局中的候选做密码学运算，不暴力枚举所有内存窗口。
                        let mut distinct = [false; 256];
                        for byte in key.iter() {
                            distinct[*byte as usize] = true;
                        }
                        if distinct.iter().filter(|v| **v).count() >= 6 {
                            report.candidates_checked += 1;
                            keys.offer(databases, &key);
                        }
                    }
                }
            }
        }
        offset += 4;
    }
}
