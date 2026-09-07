//! Windows V2 图片密钥：显式进程内存扫描与当前账号缓存的离线推导。

pub mod offline;
pub mod windows;

use anyhow::{ensure, Context, Result};
use regex::bytes::Regex;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

use crate::attachment::decoder::{detect_image_format, V2_MAGIC};

/// V2 图片真正需要的是两份材料：
/// - 16 字节 ASCII AES key
/// - XOR key（由图片内容验证）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageKeyMaterial {
    pub aes_key: [u8; 16],
    pub xor_key: u8,
}

/// 单个 wxid 的 V2 image key 提取接口。
///
/// 实现者负责跨调用缓存（一台机器上同一 wxid 的 image key 在微信不重启时通常稳定）。
pub trait ImageKeyProvider {
    fn get_key(&self, wxid: &str) -> Result<ImageKeyMaterial>;

    fn get_aes_key(&self, wxid: &str) -> Result<[u8; 16]> {
        Ok(self.get_key(wxid)?.aes_key)
    }

    fn get_xor_key(&self, wxid: &str) -> Result<u8> {
        Ok(self.get_key(wxid)?.xor_key)
    }
}

/// 平台默认实现。
pub fn default_provider() -> Option<Box<dyn ImageKeyProvider + Send + Sync>> {
    Some(Box::new(
        windows::WindowsImageKeyProvider::from_current_config(),
    ))
}

/// 连续监控可按类型重试，不能根据错误字符串判断。此错误不携带密钥或原始内存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoImageKeyFound {
    pub reason: NoImageKeyReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoImageKeyReason {
    CompletedScan,
    Deadline,
    ByteBudget,
}

impl std::fmt::Display for NoImageKeyFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.reason {
            NoImageKeyReason::CompletedScan => {
                "已完成匹配进程扫描，未找到通过固定账号模板验证的 AES-128 密钥"
            }
            NoImageKeyReason::Deadline => "本轮图片密钥提取截止时间已到，可开始新一轮有界扫描",
            NoImageKeyReason::ByteBudget => "本轮图片密钥读取字节预算耗尽，可开始新一轮有界扫描",
        })
    }
}
impl std::error::Error for NoImageKeyFound {}

/// 一次提取共享全部预算；超限返回错误，不能把截断扫描当成“没有密钥”。
pub(super) struct ExtractionBudget {
    deadline: Instant,
    bytes_left: u64,
    entries_left: usize,
    candidates_left: usize,
    regions_left: usize,
}

impl ExtractionBudget {
    pub(super) fn new(timeout: Duration, max_bytes: u64) -> Result<Self> {
        ensure!(
            !timeout.is_zero() && max_bytes > 0,
            "图片密钥提取的超时与字节预算必须大于零"
        );
        Ok(Self {
            deadline: Instant::now()
                .checked_add(timeout)
                .context("提取截止时间超出范围")?,
            bytes_left: max_bytes,
            entries_left: 100_000,
            candidates_left: 100_000,
            regions_left: 1_000_000,
        })
    }
    pub(super) fn check(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(NoImageKeyFound {
                reason: NoImageKeyReason::Deadline,
            }
            .into());
        }
        Ok(())
    }
    pub(super) fn read(&mut self, bytes: usize) -> Result<()> {
        self.check()?;
        self.bytes_left = self
            .bytes_left
            .checked_sub(bytes as u64)
            .ok_or(NoImageKeyFound {
                reason: NoImageKeyReason::ByteBudget,
            })?;
        Ok(())
    }
    pub(super) fn remaining_bytes(&self) -> u64 {
        self.bytes_left
    }
    pub(super) fn entry(&mut self) -> Result<()> {
        self.check()?;
        self.entries_left = self
            .entries_left
            .checked_sub(1)
            .context("图片密钥提取全局枚举条目预算耗尽")?;
        Ok(())
    }
    pub(super) fn candidate(&mut self) -> Result<()> {
        self.check()?;
        self.candidates_left = self
            .candidates_left
            .checked_sub(1)
            .context("图片密钥提取全局候选预算耗尽")?;
        Ok(())
    }
    pub(super) fn region(&mut self) -> Result<()> {
        self.check()?;
        self.regions_left = self
            .regions_left
            .checked_sub(1)
            .context("图片密钥提取全局内存区域预算耗尽")?;
        Ok(())
    }
}

pub(crate) fn configured_db_dir_for_wxid(
    configured_db_dir: &Path,
    requested_wxid: &str,
) -> PathBuf {
    if requested_wxid.trim().is_empty() {
        return configured_db_dir.to_path_buf();
    }

    let configured_leaf = wxid_from_db_dir(configured_db_dir);
    if let Some(leaf) = configured_leaf.as_deref() {
        if same_wxid(leaf, requested_wxid) {
            return configured_db_dir.to_path_buf();
        }
    }

    xwechat_files_root(configured_db_dir)
        .map(|root| root.join(requested_wxid).join("db_storage"))
        .unwrap_or_else(|| configured_db_dir.to_path_buf())
}

pub(crate) fn wxid_from_db_dir(db_dir: &Path) -> Option<String> {
    let mut components = db_dir
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned());
    while let Some(component) = components.next() {
        if component == "xwechat_files" {
            return components.next();
        }
    }
    None
}

pub(crate) fn xwechat_files_root(db_dir: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = db_dir
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let idx = parts.iter().position(|part| part == "xwechat_files")?;
    Some(join_components(&parts[..=idx]))
}

pub(crate) fn normalize_wxid(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Some(stripped) = raw.strip_prefix("wxid_") {
        let head = stripped.split('_').next().unwrap_or(stripped);
        return format!("wxid_{}", head);
    }
    if let Some((base, suffix)) = raw.rsplit_once('_') {
        if suffix.len() == 4 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return base.to_string();
        }
    }
    raw.to_string()
}

pub(crate) fn same_wxid(a: &str, b: &str) -> bool {
    a == b || normalize_wxid(a) == normalize_wxid(b)
}

pub(crate) fn join_components(parts: &[String]) -> PathBuf {
    let mut out = if parts.first().map(|part| part.is_empty()).unwrap_or(false) {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for part in parts {
        if part.is_empty() {
            continue;
        }
        out.push(part);
    }
    out
}

pub(crate) fn attach_root_for_db_dir(db_dir: &Path) -> PathBuf {
    db_dir
        .parent()
        .map(|base| base.join("msg").join("attach"))
        .unwrap_or_else(|| PathBuf::from("msg/attach"))
}

pub(crate) fn find_v2_template_ciphertexts(
    attach_dir: &Path,
    max_templates: usize,
    max_files: usize,
) -> Result<Vec<[u8; 16]>> {
    let mut budget = ExtractionBudget::new(Duration::from_secs(30), 16 * 1024 * 1024)?;
    find_templates_bounded(attach_dir, max_templates, max_files, &mut budget)
}

pub(super) fn find_templates_bounded(
    attach_dir: &Path,
    max_templates: usize,
    max_files: usize,
    budget: &mut ExtractionBudget,
) -> Result<Vec<[u8; 16]>> {
    ensure!(max_templates > 0 && max_files > 0, "模板采样数量必须大于零");
    if missing_directory(attach_dir, budget)? {
        return Ok(Vec::new());
    }
    let mut files_left = max_files;
    let mut out = collect_templates_with_suffix(
        attach_dir,
        "_t.dat",
        max_templates,
        &mut files_left,
        budget,
    )?;
    if out.is_empty() {
        out = collect_templates_with_suffix(
            attach_dir,
            ".dat",
            max_templates,
            &mut files_left,
            budget,
        )?;
    }
    Ok(out)
}

pub(crate) fn derive_xor_key_from_v2_dat(
    attach_dir: &Path,
    sample: usize,
    min_samples: usize,
) -> Result<Option<(u8, usize, usize)>> {
    let mut budget = ExtractionBudget::new(Duration::from_secs(30), 16 * 1024 * 1024)?;
    derive_xor_bounded(attach_dir, sample, min_samples, &mut budget)
}

pub(super) fn derive_xor_bounded(
    attach_dir: &Path,
    sample: usize,
    min_samples: usize,
    budget: &mut ExtractionBudget,
) -> Result<Option<(u8, usize, usize)>> {
    ensure!(
        sample > 0 && min_samples > 0 && min_samples <= sample,
        "XOR 采样数量无效"
    );
    if missing_directory(attach_dir, budget)? {
        return Ok(None);
    }
    let mut votes = Vec::new();
    visit_files(attach_dir, budget, &mut |path, budget| -> Result<bool> {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return Ok(false);
        };
        if !name.ends_with(".dat") {
            return Ok(false);
        }

        let (bytes, tail) = read_sample(path, 6, true, budget)?;
        if let Some(last) = tail.filter(|_| bytes.starts_with(&V2_MAGIC)) {
            votes.push(last ^ 0xD9);
            if votes.len() >= sample {
                return Ok(true);
            }
        }
        Ok(false)
    })?;

    if votes.len() < min_samples {
        return Ok(None);
    }

    let mut counts = [0usize; 256];
    for vote in &votes {
        counts[*vote as usize] += 1;
    }
    let (xor_key, top_votes) = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)
        .map(|(idx, count)| (idx as u8, *count))
        .expect("votes 非空");
    Ok(Some((xor_key, top_votes, votes.len())))
}

pub(crate) fn verify_aes_key(aes_key: &[u8; 16], templates: &[[u8; 16]]) -> bool {
    !templates.is_empty()
        && templates
            .iter()
            .all(|template| decrypt_template_block(aes_key, template).is_some())
}

pub(crate) fn ascii_alnum_candidates<'a>(buf: &'a [u8], len: usize) -> Vec<&'a [u8]> {
    ascii_alnum_matches(buf, len)
        .map(|range| &buf[range])
        .collect()
}

// 生产扫描逐个消费匹配，避免先分配整块内存的所有候选引用。
pub(super) fn ascii_alnum_matches(
    buf: &[u8],
    len: usize,
) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let re = match len {
        16 => Some(regex16()),
        32 => Some(regex32()),
        _ => None,
    };
    re.into_iter()
        .flat_map(move |re| re.find_iter(buf))
        .filter_map(move |matched| {
            let start = matched.start();
            let end = matched.end();
            let left_ok = start == 0 || !buf[start - 1].is_ascii_alphanumeric();
            let right_ok = end == buf.len() || !buf[end].is_ascii_alphanumeric();
            (left_ok && right_ok).then_some(start..end)
        })
}

fn collect_templates_with_suffix(
    dir: &Path,
    suffix: &str,
    max_templates: usize,
    files_left: &mut usize,
    budget: &mut ExtractionBudget,
) -> Result<Vec<[u8; 16]>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    visit_files(dir, budget, &mut |path, budget| -> Result<bool> {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return Ok(false);
        };
        if !name.ends_with(suffix) {
            return Ok(false);
        }
        *files_left = files_left
            .checked_sub(1)
            .context("V2 模板文件采样预算耗尽，未交付截断结果")?;
        let (bytes, _) = read_sample(path, 31, false, budget)?;
        if bytes.len() >= 0x1F && bytes.starts_with(&V2_MAGIC) {
            let template: [u8; 16] = bytes[0x0F..0x1F].try_into().unwrap();
            if seen.insert(template) {
                out.push(template);
                if out.len() >= max_templates {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    })?;
    Ok(out)
}

fn missing_directory(dir: &Path, budget: &ExtractionBudget) -> Result<bool> {
    budget.check()?;
    match fs::symlink_metadata(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
        Ok(meta) => {
            safe_metadata(&meta)?;
            ensure!(meta.is_dir(), "图片附件根不是目录");
            Ok(false)
        }
    }
}

fn safe_metadata(meta: &fs::Metadata) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "图片采样拒绝目录联接或重解析点"
        );
    }
    ensure!(
        !meta.file_type().is_symlink() && (meta.is_file() || meta.is_dir()),
        "图片采样只允许常规目录和文件"
    );
    Ok(())
}

fn read_sample(
    path: &Path,
    prefix_len: usize,
    with_tail: bool,
    budget: &mut ExtractionBudget,
) -> Result<(Zeroizing<Vec<u8>>, Option<u8>)> {
    budget.check()?;
    let pin = crate::attachment::local_files::Pin::open(path, false)?;
    let len = pin.file.metadata()?.len();
    let prefix_len = (prefix_len as u64).min(len) as usize;
    let tail = with_tail && len >= 0x20;
    budget.read(prefix_len + usize::from(tail))?;
    let mut prefix = Zeroizing::new(vec![0u8; prefix_len]);
    let mut file = &pin.file;
    file.read_exact(&mut prefix)?;
    let last = if tail {
        budget.check()?;
        file.seek(SeekFrom::End(-1))?;
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte)?;
        Some(byte[0])
    } else {
        None
    };
    pin.verify()?;
    budget.check()?;
    Ok((prefix, last))
}

fn visit_files<F>(dir: &Path, budget: &mut ExtractionBudget, f: &mut F) -> Result<bool>
where
    F: FnMut(&Path, &mut ExtractionBudget) -> Result<bool>,
{
    // 先固定全部已有祖先，递归时只开直属目录；不通过 canonicalize 隐藏重解析点。
    let mut root = crate::attachment::local_files::Scan::new();
    root.root(dir)?;
    fn walk<F>(dir: &Path, depth: usize, budget: &mut ExtractionBudget, f: &mut F) -> Result<bool>
    where
        F: FnMut(&Path, &mut ExtractionBudget) -> Result<bool>,
    {
        budget.check()?;
        ensure!(depth <= 16, "图片采样目录深度超过 16 层");
        let guard = crate::attachment::local_files::Pin::open(dir, true)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            budget.entry()?;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            safe_metadata(&meta)?;
            entries.push((entry.path(), meta.is_dir()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, is_dir) in entries {
            budget.check()?;
            let done = if is_dir {
                walk(&path, depth + 1, budget, f)?
            } else {
                f(&path, budget)?
            };
            if done {
                guard.verify()?;
                budget.check()?;
                return Ok(true);
            }
        }
        guard.verify()?;
        budget.check()?;
        Ok(false)
    }
    let done = walk(dir, 0, budget, f)?;
    root.verify()?;
    budget.check()?;
    Ok(done)
}

fn decrypt_template_block(aes_key: &[u8; 16], ciphertext: &[u8; 16]) -> Option<&'static str> {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};

    let cipher = aes::Aes128::new(aes_key.into());
    let mut block = GenericArray::clone_from_slice(ciphertext);
    cipher.decrypt_block(&mut block);
    let format = detect_image_format(block.as_slice());
    block.as_mut_slice().zeroize();
    (format != "bin").then_some(format)
}

fn regex16() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9]{16}").unwrap())
}

fn regex32() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9]{32}").unwrap())
}

#[cfg(test)]
mod tests {
    use super::{ascii_alnum_candidates, normalize_wxid, same_wxid};

    #[test]
    fn regex_candidates_respect_boundaries() {
        let buf = b"xx 0123456789ABCDef yy";
        let hits = ascii_alnum_candidates(buf, 16);
        assert_eq!(hits, vec![&buf[3..19]]);
    }

    #[test]
    fn regex_candidates_ignore_embedded_runs() {
        let buf = b"x0123456789ABCDefz";
        assert!(ascii_alnum_candidates(buf, 16).is_empty());
    }

    #[test]
    fn wxid_normalization_matches_expected_forms() {
        assert_eq!(normalize_wxid("wxid_abc_def"), "wxid_abc");
        assert_eq!(normalize_wxid("your_wxid_a1b2"), "your_wxid");
        assert!(same_wxid("your_wxid_a1b2", "your_wxid"));
    }
}
