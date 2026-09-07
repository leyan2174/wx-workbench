//! 针对显式指定的标准账号目录，以只读方式推断图片密钥。
//! 仅实现 find_all_keys.py 的本地图片推断步骤，不代表数据库密钥也能离线提取。

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use anyhow::{ensure, Context, Result};
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::{Component, Path, PathBuf, Prefix},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};
use zeroize::{Zeroize, Zeroizing};

use super::ImageKeyMaterial;
use crate::attachment::{
    decoder::{v2, V2KeyMaterial, V2_MAGIC},
    local_files::{safe_name, HostOutputGuard, Pin, Scan},
};

const HIGH_UIN_COUNT: u32 = 1 << 24;
const MAX_ENTRIES: usize = 100_000;
const MAX_THUMBNAILS: usize = 100;
const XOR_THUMBNAILS: usize = 32;
const MAX_VALIDATION_SAMPLES: usize = 3;
const MAX_SAMPLE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoMatchReason {
    UnsupportedAccountName,
    NoThumbnailSamples,
    NoXorEvidence,
    NoV2Sample,
    CompletedSearch,
    Deadline,
    ByteBudget,
    EntryBudget,
    SampleTooLarge,
    Cancelled,
}

/// 未返回密钥；预算耗尽或取消不表示已经遍历全部候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMatch {
    pub reason: NoMatchReason,
    pub candidates_tested: u32,
}

impl std::fmt::Display for NoMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let description = match self.reason {
            NoMatchReason::UnsupportedAccountName => {
                "账号目录名须包含 ASCII 基础账号名和四位十六进制后缀"
            }
            NoMatchReason::NoThumbnailSamples => "所选账号的 */*/Img/*_t.dat 路径下没有缩略图样本",
            NoMatchReason::NoXorEvidence => {
                "出现次数最多的缩略图尾部双字节不符合旧版 JPEG XOR 推导规则"
            }
            NoMatchReason::NoV2Sample => "最新 100 个缩略图中没有完整的 V2 密文块",
            NoMatchReason::CompletedSearch => "已遍历候选范围，未找到通过样本验证的密钥",
            NoMatchReason::Deadline => "已到截止时间，搜索尚未完成",
            NoMatchReason::ByteBudget => "样本读取字节预算已耗尽，搜索尚未完成",
            NoMatchReason::EntryBudget => "缩略图枚举达到条目上限，样本选择尚未完成",
            NoMatchReason::SampleTooLarge => "验证样本超过单文件 8 MiB 上限",
            NoMatchReason::Cancelled => "操作已取消，搜索尚未完成",
        };
        write!(
            f,
            "离线图片密钥推断：{description}（已检查候选数：{}）",
            self.candidates_tested
        )
    }
}
impl std::error::Error for NoMatch {}

struct Budget<'a> {
    deadline: Instant,
    bytes_left: u64,
    entries_left: usize,
    candidates_tested: u32,
    cancelled: &'a AtomicBool,
}

impl<'a> Budget<'a> {
    fn new(timeout: Duration, max_bytes: u64, cancelled: &'a AtomicBool) -> Result<Self> {
        ensure!(
            !timeout.is_zero() && max_bytes > 0,
            "离线推断的超时时间和字节预算必须大于零"
        );
        Ok(Self {
            deadline: Instant::now()
                .checked_add(timeout)
                .context("离线推断截止时间超出范围")?,
            bytes_left: max_bytes,
            entries_left: MAX_ENTRIES,
            candidates_tested: 0,
            cancelled,
        })
    }

    fn no_match(&self, reason: NoMatchReason) -> anyhow::Error {
        NoMatch {
            reason,
            candidates_tested: self.candidates_tested,
        }
        .into()
    }

    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(self.no_match(NoMatchReason::Cancelled));
        }
        if Instant::now() >= self.deadline {
            return Err(self.no_match(NoMatchReason::Deadline));
        }
        Ok(())
    }

    fn read(&mut self, bytes: u64) -> Result<()> {
        self.check()?;
        self.bytes_left = self
            .bytes_left
            .checked_sub(bytes)
            .ok_or_else(|| self.no_match(NoMatchReason::ByteBudget))?;
        Ok(())
    }

    fn entry(&mut self) -> Result<()> {
        self.check()?;
        self.entries_left = self
            .entries_left
            .checked_sub(1)
            .ok_or_else(|| self.no_match(NoMatchReason::EntryBudget))?;
        Ok(())
    }
}

struct Account {
    base: Vec<u8>,
    suffix: [u8; 2],
}

fn account_name(folder: &str) -> Option<Account> {
    let (base, suffix) = folder.rsplit_once('_')?;
    if base.is_empty()
        || !base.is_ascii()
        || base.len() > 255
        || suffix.len() != 4
        || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(Account {
        base: base.as_bytes().to_vec(),
        suffix: [
            u8::from_str_radix(&suffix[..2], 16).ok()?,
            u8::from_str_radix(&suffix[2..], 16).ok()?,
        ],
    })
}

fn validate_db_path(db_dir: &Path) -> Result<()> {
    ensure!(
        db_dir.is_absolute(),
        "离线推断的 db_dir 必须是显式指定的本机绝对路径"
    );
    let raw = db_dir.to_str().context("db_dir 路径编码无效")?;
    ensure!(
        raw.len() <= 32760
            && !raw
                .split(['/', '\\'])
                .any(|part| part == "." || part == ".."),
        "db_dir 路径过长或包含不安全的分量"
    );
    for component in db_dir.components() {
        match component {
            Component::Prefix(prefix) => ensure!(
                matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
                "离线推断只接受本机磁盘路径"
            ),
            Component::Normal(name) => {
                safe_name(name.to_str().context("db_dir 路径分量编码无效")?)?
            }
            Component::RootDir => {}
            _ => anyhow::bail!("db_dir 包含不安全的路径分量"),
        }
    }
    Ok(())
}

type RankedPath = (SystemTime, PathBuf);

fn remember_latest(heap: &mut BinaryHeap<Reverse<RankedPath>>, entry: RankedPath) {
    heap.push(Reverse(entry));
    if heap.len() > MAX_THUMBNAILS {
        heap.pop();
    }
}

// 仅匹配 attach/*/*/Img/*_t.dat，不递归搜索整个账号目录或全局目录。
fn thumbnails(
    dir: &Path,
    level: usize,
    heap: &mut BinaryHeap<Reverse<RankedPath>>,
    budget: &mut Budget<'_>,
) -> Result<()> {
    budget.check()?;
    let guard = Pin::open(dir, true)?;
    for entry in fs::read_dir(dir)? {
        budget.entry()?;
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("缩略图目录项名称编码无效")?;
        safe_name(name)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        super::safe_metadata(&metadata)?;
        if level < 2 && metadata.is_dir()
            || level == 2 && name.eq_ignore_ascii_case("Img") && metadata.is_dir()
        {
            thumbnails(&path, level + 1, heap, budget)?;
        } else if level == 3 && metadata.is_file() && name.to_ascii_lowercase().ends_with("_t.dat")
        {
            remember_latest(heap, (metadata.modified()?, path));
        }
    }
    guard.verify()?;
    budget.check()
}

struct Sample {
    pin: Pin,
    guard: HostOutputGuard,
    header: Vec<u8>,
    tail: Option<[u8; 2]>,
    len: u64,
}

impl Sample {
    fn open(
        path: &Path,
        modified: SystemTime,
        with_tail: bool,
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        budget.check()?;
        let guard = HostOutputGuard::new(path.parent().context("缩略图路径缺少父目录")?)?;
        guard.verify_replaceable_file(path)?;
        let pin = Pin::open(path, false)?;
        let metadata = pin.file.metadata()?;
        ensure!(metadata.modified()? == modified, "缩略图在选定后发生变化");
        let len = metadata.len();
        let header_len = len.min(31) as usize;
        let has_tail = with_tail && len >= 2;
        budget.read(header_len as u64 + if has_tail { 2 } else { 0 })?;
        let mut file = &pin.file;
        let mut header = vec![0; header_len];
        file.read_exact(&mut header)?;
        let tail = if has_tail {
            budget.check()?;
            file.seek(SeekFrom::End(-2))?;
            let mut tail = [0; 2];
            file.read_exact(&mut tail)?;
            Some(tail)
        } else {
            None
        };
        pin.verify()?;
        guard.verify()?;
        budget.check()?;
        Ok(Self {
            pin,
            guard,
            header,
            tail,
            len,
        })
    }

    fn body(&self, budget: &mut Budget<'_>) -> Result<Zeroizing<Vec<u8>>> {
        budget.check()?;
        if self.len > MAX_SAMPLE_BYTES {
            return Err(budget.no_match(NoMatchReason::SampleTooLarge));
        }
        budget.read(self.len)?;
        let mut bytes = Zeroizing::new(vec![0; self.len as usize]);
        let mut file = &self.pin.file;
        file.seek(SeekFrom::Start(0))?;
        // 分块读取，在读取样本期间也能检查取消信号。
        for chunk in bytes.chunks_mut(64 * 1024) {
            budget.check()?;
            file.read_exact(chunk)?;
        }
        self.pin.verify()?;
        self.guard.verify()?;
        budget.check()?;
        Ok(bytes)
    }
}

fn xor_from_tails(tails: impl IntoIterator<Item = [u8; 2]>) -> Option<u8> {
    let mut counts: Vec<([u8; 2], usize)> = Vec::new();
    for tail in tails {
        if let Some((_, count)) = counts.iter_mut().find(|(value, _)| *value == tail) {
            *count += 1;
        } else {
            counts.push((tail, 1));
        }
    }
    // 与 Python 的 max(dict, key=...) 一致：票数相同时保留最先出现的双字节。
    let mut winner = None;
    let mut best = 0;
    for (tail, count) in counts {
        if count > best {
            best = count;
            winner = Some(tail);
        }
    }
    let [x, y] = winner?;
    let xor = x ^ 0xff;
    (y ^ 0xd9 == xor).then_some(xor)
}

fn decimal(uin: u32, buffer: &mut [u8; 10]) -> &[u8] {
    let mut value = uin;
    let mut offset = buffer.len();
    loop {
        offset -= 1;
        buffer[offset] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    &buffer[offset..]
}

fn aes_candidate(uin: &[u8], base: &[u8]) -> Zeroizing<[u8; 16]> {
    let mut digest = md5::Context::new();
    digest.consume(uin);
    digest.consume(base);
    let digest = Zeroizing::new(digest.compute().0);
    let mut key = Zeroizing::new([0; 16]);
    let hex = b"0123456789abcdef";
    for (index, byte) in digest[..8].iter().enumerate() {
        key[index * 2] = hex[(byte >> 4) as usize];
        key[index * 2 + 1] = hex[(byte & 15) as usize];
    }
    key
}

fn legacy_template_matches(key: &[u8; 16], ciphertext: &[u8; 16]) -> bool {
    let mut block = GenericArray::clone_from_slice(ciphertext);
    aes::Aes128::new(key.into()).decrypt_block(&mut block);
    let valid = block.starts_with(&[0xff, 0xd8, 0xff])
        || block.starts_with(b"\x89PNG")
        || block.starts_with(b"RIFF")
        || block.starts_with(b"wxgf")
        || block.starts_with(b"GIF");
    block.as_mut_slice().zeroize();
    valid
}

fn validate_bodies(
    bodies: &[Zeroizing<Vec<u8>>],
    key: &[u8; 16],
    xor_key: u8,
    budget: &Budget<'_>,
) -> Result<bool> {
    ensure!(!bodies.is_empty(), "离线验证必须提供实际图片样本");
    for body in bodies {
        budget.check()?;
        let decoded = v2::decode(
            body,
            V2KeyMaterial {
                aes_key: Some(key),
                xor_key,
            },
        );
        let valid = match decoded {
            Ok(mut image) => {
                image.data.zeroize();
                true
            }
            Err(_) => false,
        };
        budget.check()?;
        if !valid {
            return Ok(false);
        }
    }
    Ok(true)
}

// 范围参数仅供内部合成测试缩小搜索量，生产入口始终传入 0..2^24。
fn search_range(
    account: &Account,
    xor_key: u8,
    ciphertext: &[u8; 16],
    range: Range<u32>,
    budget: &mut Budget<'_>,
    mut validate: impl FnMut(&[u8; 16], &mut Budget<'_>) -> Result<bool>,
) -> Result<ImageKeyMaterial> {
    ensure!(
        range.start <= range.end && range.end <= HIGH_UIN_COUNT,
        "离线推断候选范围无效"
    );
    budget.check()?;
    let mut digits = [0; 10];
    for high in range {
        if budget.candidates_tested % 1024 == 0 {
            budget.check()?;
        }
        budget.candidates_tested += 1;
        let uin = (high << 8) | u32::from(xor_key);
        let text = decimal(uin, &mut digits);
        if md5::compute(text).0[..2] != account.suffix {
            continue;
        }
        let candidate = aes_candidate(text, &account.base);
        if legacy_template_matches(&candidate, ciphertext) && validate(&candidate, budget)? {
            budget.check()?;
            return Ok(ImageKeyMaterial {
                aes_key: *candidate,
                xor_key,
            });
        }
    }
    budget.check()?;
    Err(budget.no_match(NoMatchReason::CompletedSearch))
}

/// 仅读取显式选定账号的样本，不读取配置或进程、不发现其他账号，也不写入文件。
/// 字节预算计入头尾读取请求和完整样本重读，包括失败的读取请求。
/// 可遍历完整的 2^24 个 UIN 高位候选，不在 10 万个候选处截断。
/// 每 1024 个 MD5 候选及读写、解码调用前后检查取消信号与截止时间。
/// 正在进行的文件系统或解码调用采用协作式取消，不能被强制中断。
pub fn extract_for_db_dir(
    db_dir: &Path,
    timeout: Duration,
    max_bytes: u64,
    cancelled: &AtomicBool,
) -> Result<ImageKeyMaterial> {
    let mut budget = Budget::new(timeout, max_bytes, cancelled)?;
    budget.check()?;
    validate_db_path(db_dir)?;
    let parent = db_dir.parent().context("db_dir 缺少账号父目录")?;
    let account = parent
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(account_name)
        .ok_or_else(|| budget.no_match(NoMatchReason::UnsupportedAccountName))?;
    let mut source = Scan::new();
    source.root(db_dir)?;
    budget.check()?;
    let mut attach = parent.to_path_buf();
    let mut attach_pins = Vec::new();
    for name in ["msg", "attach"] {
        attach.push(name);
        budget.check()?;
        match Pin::open(&attach, true) {
            Ok(pin) => attach_pins.push(pin),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Err(budget.no_match(NoMatchReason::NoThumbnailSamples));
            }
            Err(error) => return Err(error),
        }
    }
    let mut heap = BinaryHeap::new();
    thumbnails(&attach, 0, &mut heap, &mut budget)?;
    let mut paths: Vec<_> = heap.into_iter().map(|Reverse(entry)| entry).collect();
    paths.sort_by(|a, b| b.cmp(a));
    if paths.is_empty() {
        return Err(budget.no_match(NoMatchReason::NoThumbnailSamples));
    }
    let mut samples = Vec::new();
    for (index, (modified, path)) in paths.iter().enumerate() {
        samples.push(Sample::open(
            path,
            *modified,
            index < XOR_THUMBNAILS,
            &mut budget,
        )?);
    }
    let xor_key = xor_from_tails(
        samples
            .iter()
            .take(XOR_THUMBNAILS)
            .filter(|sample| sample.header.starts_with(&V2_MAGIC))
            .filter_map(|sample| sample.tail),
    )
    .ok_or_else(|| budget.no_match(NoMatchReason::NoXorEvidence))?;
    let templates: Vec<_> = samples
        .iter()
        .filter(|sample| sample.header.len() >= 31 && sample.header.starts_with(&V2_MAGIC))
        .take(MAX_VALIDATION_SAMPLES)
        .collect();
    let first = templates
        .first()
        .ok_or_else(|| budget.no_match(NoMatchReason::NoV2Sample))?;
    let ciphertext: [u8; 16] = first.header[15..31].try_into().unwrap();
    let mut bodies = None;
    let material = search_range(
        &account,
        xor_key,
        &ciphertext,
        0..HIGH_UIN_COUNT,
        &mut budget,
        |key, budget| {
            if bodies.is_none() {
                bodies = Some(
                    templates
                        .iter()
                        .map(|sample| sample.body(budget))
                        .collect::<Result<Vec<_>>>()?,
                );
            }
            validate_bodies(bodies.as_ref().unwrap(), key, xor_key, budget)
        },
    )?;
    // 在整个推断期间持续持有所选样本和账号目录的固定句柄。
    source.verify()?;
    for pin in &attach_pins {
        pin.verify()?;
    }
    for sample in &samples {
        sample.pin.verify()?;
        sample.guard.verify()?;
    }
    budget.check()?;
    Ok(material)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    fn synthetic(uin: u32, base: &str) -> (Account, [u8; 16], Vec<u8>) {
        let text = uin.to_string();
        let digest = md5::compute(text.as_bytes());
        let account = Account {
            base: base.as_bytes().to_vec(),
            suffix: digest.0[..2].try_into().unwrap(),
        };
        let reference = format!("{:x}", md5::compute(format!("{uin}{base}")));
        let key: [u8; 16] = reference.as_bytes()[..16].try_into().unwrap();
        let mut padded = [13; 16];
        padded[..3].copy_from_slice(&[0xff, 0xd8, 0xff]);
        let mut block = GenericArray::clone_from_slice(&padded);
        aes::Aes128::new((&key).into()).encrypt_block(&mut block);
        let mut bytes = V2_MAGIC.to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&block);
        bytes.extend_from_slice(&[0xff ^ uin as u8, 0xd9 ^ uin as u8]);
        (account, key, bytes)
    }

    fn reason(error: anyhow::Error) -> NoMatchReason {
        error.downcast::<NoMatch>().unwrap().reason
    }

    #[test]
    fn derivation_matches_legacy_decimal_md5_ascii_hex_semantics() {
        for uin in [0, 255, 256, u32::MAX] {
            let (_, expected, _) = synthetic(uin, "wxid_base_with_underscores");
            let mut buffer = [0; 10];
            assert_eq!(decimal(uin, &mut buffer), uin.to_string().as_bytes());
            assert_eq!(
                *aes_candidate(decimal(uin, &mut buffer), b"wxid_base_with_underscores"),
                expected
            );
        }
        assert_eq!(HIGH_UIN_COUNT, 16_777_216);
    }

    #[test]
    fn account_suffix_parser_preserves_entire_base_and_accepts_upper_hex() {
        let account = account_name("wxid_base_with_underscores_CFCD").unwrap();
        assert_eq!(account.base, b"wxid_base_with_underscores");
        assert_eq!(account.suffix, [0xcf, 0xcd]);
        for invalid in ["renamed", "_1234", "wxid_123", "wxid_zzzz", "wxid_12345"] {
            assert!(account_name(invalid).is_none());
        }
    }

    #[test]
    fn xor_vote_uses_pair_majority_first_tie_and_no_invalid_winner_fallback() {
        let a = [0xff ^ 0x88, 0xd9 ^ 0x88];
        let b = [0xff ^ 0x31, 0xd9 ^ 0x31];
        assert_eq!(xor_from_tails([a, b]), Some(0x88));
        assert_eq!(xor_from_tails([a, b, b]), Some(0x31));
        assert_eq!(xor_from_tails([[0, 0], a, [0, 0]]), None);
        assert_eq!(xor_from_tails([]), None);
    }

    #[test]
    fn last_uin_candidate_is_reachable_and_requires_actual_validation() -> Result<()> {
        let (account, expected, bytes) = synthetic(u32::MAX, "wxid_fixture");
        let ciphertext = bytes[15..31].try_into()?;
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new(Duration::from_secs(10), 4096, &cancelled)?;
        let bodies = vec![Zeroizing::new(bytes)];
        let result = search_range(
            &account,
            255,
            &ciphertext,
            HIGH_UIN_COUNT - 1..HIGH_UIN_COUNT,
            &mut budget,
            |key, budget| validate_bodies(&bodies, key, 255, budget),
        )?;
        assert_eq!(result.aes_key, expected);
        assert_eq!(result.xor_key, 255);
        assert_eq!(budget.candidates_tested, 1);
        Ok(())
    }

    #[test]
    fn suffix_and_image_prefix_alone_never_return_a_key() -> Result<()> {
        let (account, _, bytes) = synthetic(0, "wxid_fixture");
        let ciphertext = bytes[15..31].try_into()?;
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new(Duration::from_secs(10), 4096, &cancelled)?;
        let error = search_range(&account, 0, &ciphertext, 0..1, &mut budget, |_, _| {
            Ok(false)
        })
        .unwrap_err();
        assert_eq!(reason(error), NoMatchReason::CompletedSearch);
        assert_eq!(budget.candidates_tested, 1);
        Ok(())
    }

    #[test]
    fn full_sample_validation_rejects_bad_xor_tail() -> Result<()> {
        let (_, key, mut bytes) = synthetic(0, "wxid_fixture");
        *bytes.last_mut().unwrap() ^= 1;
        let cancelled = AtomicBool::new(false);
        let budget = Budget::new(Duration::from_secs(10), 4096, &cancelled)?;
        assert!(!validate_bodies(
            &[Zeroizing::new(bytes)],
            &key,
            0,
            &budget
        )?);
        Ok(())
    }

    #[test]
    fn budgets_report_distinct_incomplete_reasons() -> Result<()> {
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new(Duration::from_secs(10), 8, &cancelled)?;
        assert_eq!(
            reason(budget.read(9).unwrap_err()),
            NoMatchReason::ByteBudget
        );
        budget.entries_left = 0;
        assert_eq!(
            reason(budget.entry().unwrap_err()),
            NoMatchReason::EntryBudget
        );
        budget.deadline = Instant::now();
        assert_eq!(reason(budget.check().unwrap_err()), NoMatchReason::Deadline);
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(
            reason(budget.check().unwrap_err()),
            NoMatchReason::Cancelled
        );
        Ok(())
    }

    #[test]
    fn latest_selection_is_bounded_and_keeps_newest_hundred() {
        let mut heap = BinaryHeap::new();
        for index in 0..150 {
            remember_latest(
                &mut heap,
                (
                    SystemTime::UNIX_EPOCH + Duration::from_secs(index),
                    PathBuf::from(format!("{index}_t.dat")),
                ),
            );
        }
        assert_eq!(heap.len(), 100);
        assert_eq!(
            heap.peek().unwrap().0 .0,
            SystemTime::UNIX_EPOCH + Duration::from_secs(50)
        );
    }

    #[test]
    fn explicit_temporary_account_infers_without_process_or_config_access() -> Result<()> {
        let temp = tempfile::Builder::new()
            .prefix("wx-offline-test-")
            .tempdir()?;
        let (_, expected, bytes) = synthetic(0, "wxid_fixture");
        let account = temp.path().join("wxid_fixture_cfcd");
        let db_dir = account.join("db_storage");
        let images = account.join("msg/attach/contact/month/Img");
        fs::create_dir_all(&db_dir)?;
        fs::create_dir_all(&images)?;
        let sample = images.join("fixture_t.dat");
        fs::write(&sample, &bytes)?;
        let cancelled = AtomicBool::new(false);
        let result = extract_for_db_dir(&db_dir, Duration::from_secs(10), 4096, &cancelled)?;
        assert_eq!(result.aes_key, expected);
        assert_eq!(result.xor_key, 0);
        assert_eq!(fs::read(&sample)?, bytes);
        assert!(!account.join("config.json").exists());
        assert_eq!(fs::read_dir(&db_dir)?.count(), 0);
        Ok(())
    }

    #[test]
    fn files_outside_legacy_thumbnail_layout_are_not_samples() -> Result<()> {
        let temp = tempfile::Builder::new()
            .prefix("wx-offline-test-")
            .tempdir()?;
        let (_, _, bytes) = synthetic(0, "wxid_fixture");
        let account = temp.path().join("wxid_fixture_cfcd");
        let db_dir = account.join("db_storage");
        let attach = account.join("msg/attach");
        fs::create_dir_all(&db_dir)?;
        fs::create_dir_all(&attach)?;
        fs::write(attach.join("outside_t.dat"), bytes)?;
        let error = extract_for_db_dir(
            &db_dir,
            Duration::from_secs(10),
            4096,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(reason(error), NoMatchReason::NoThumbnailSamples);
        Ok(())
    }
}
