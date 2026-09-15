//! Controlled attachment IO: bounded enumeration, pinned handles and verified reads.
//! Legacy metadata projections are re-exported for existing protocol callers.
use crate::adapters::wechat::media::attachment_content::{
    cache_ancestors, file_match, image_match, record_collection, record_item_directory,
    record_media, safe_name, validate_metadata,
};
pub use crate::adapters::wechat::media::attachment_content::{
    parse_file_message, parse_record_item, AttachmentMetadata, Error, ErrorKind, Kind,
    MessageInput, Result,
};
use crate::business::attachment_content::AttachmentContent;
use serde::Serialize;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Seek},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};
pub const MAX_HASH_BYTES: u64 = 500 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_DIRECTORIES: usize = 1024;
const MAX_CANDIDATES: usize = 128;
const MAX_DEPTH: usize = 16;
fn error(kind: ErrorKind, stage: &'static str) -> Error {
    Error { kind, stage }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Md5,
    Heuristic,
}

#[derive(Debug, Serialize)]
pub struct FileReference {
    pub path: PathBuf,
    pub size: u64,
    pub md5: String,
    pub binding: Binding,
    pub warning: Option<&'static str>,
    pub equivalent_copies: usize,
    #[serde(skip)]
    file: Pin,
    #[serde(skip)]
    _directories: Vec<Pin>,
}
impl FileReference {
    /// 引用存活期间保持只读句柄；序列化路径本身不延长锁，也不是账号认证凭证。
    pub fn file(&self) -> &File {
        &self.file.file
    }
}

#[derive(Debug)]
struct Pin {
    path: PathBuf,
    file: File,
    id: same_file::Handle,
    stamp: Stamp,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    directory: bool,
    size: u64,
    modified: SystemTime,
}
fn stamp(meta: &Metadata) -> Result<Stamp> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(error(ErrorKind::UnsafePath, "拒绝 reparse point"));
        }
    }
    if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) {
        return Err(error(ErrorKind::UnsafePath, "只允许常规文件和目录"));
    }
    Ok(Stamp {
        directory: meta.is_dir(),
        size: meta.len(),
        modified: meta
            .modified()
            .map_err(|_| error(ErrorKind::Io, "无法读取文件时间"))?,
    })
}
impl Pin {
    fn open(path: &Path, directory: bool) -> Result<Option<Self>> {
        let mut options = OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 目录只读属性，文件只读内容；不跟随 reparse，并拒绝并发写入与删除句柄。
            options
                .access_mode(if directory { 0x80 } else { 0x80000000 })
                .share_mode(1)
                .custom_flags(0x02200000);
        }
        #[cfg(not(windows))]
        {
            options.read(true);
        }
        let before = match fs::symlink_metadata(path) {
            Ok(m) => stamp(&m)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(error(ErrorKind::Io, "无法读取路径属性")),
        };
        if before.directory != directory {
            return Err(error(ErrorKind::UnsafePath, "文件与目录类型不符"));
        }
        let file = options
            .open(path)
            .map_err(|_| error(ErrorKind::Io, "无法固定只读句柄"))?;
        let current = stamp(
            &file
                .metadata()
                .map_err(|_| error(ErrorKind::Io, "无法读取句柄属性"))?,
        )?;
        if before.directory != current.directory || (!directory && before != current) {
            return Err(error(ErrorKind::Changed, "打开期间文件发生变化"));
        }
        let id = same_file::Handle::from_file(
            file.try_clone()
                .map_err(|_| error(ErrorKind::Io, "复制句柄失败"))?,
        )
        .map_err(|_| error(ErrorKind::Io, "读取文件身份失败"))?;
        let pin = Self {
            path: path.into(),
            file,
            id,
            stamp: current,
        };
        pin.verify()?;
        Ok(Some(pin))
    }
    fn verify(&self) -> Result<()> {
        let current = stamp(
            &fs::symlink_metadata(&self.path)
                .map_err(|_| error(ErrorKind::Changed, "路径不再可读"))?,
        )?;
        let id = same_file::Handle::from_path(&self.path)
            .map_err(|_| error(ErrorKind::Changed, "路径身份不可读"))?;
        if self.id != id
            || self.stamp.directory != current.directory
            || (!current.directory && self.stamp != current)
        {
            return Err(error(ErrorKind::Changed, "文件身份或内容属性发生变化"));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    stamp: Stamp,
}
struct Snapshot {
    path: PathBuf,
    entries: Vec<Entry>,
}
struct Scan {
    entries_left: usize,
    hash_left: u64,
    pins: Vec<Pin>,
    snapshots: Vec<Snapshot>,
    missing: Vec<PathBuf>,
}
impl Scan {
    fn new(base: &Path) -> Result<Self> {
        if !base.is_absolute() || base.as_os_str().len() > 32760 {
            return Err(error(ErrorKind::UnsafePath, "base 必须是本机显式绝对目录"));
        }
        // components 会折叠部分 '.'；先检查原始输入，不能借归一化绕过。
        if base
            .to_str()
            .is_none_or(|s| s.split(['/', '\\']).any(|c| c == "." || c == ".."))
        {
            return Err(error(ErrorKind::UnsafePath, "路径含相对组件或无效编码"));
        }
        for part in base.components() {
            match part {
                Component::Normal(s) => safe_name(
                    s.to_str()
                        .ok_or_else(|| error(ErrorKind::UnsafePath, "路径编码无效"))?,
                )?,
                Component::ParentDir | Component::CurDir => {
                    return Err(error(ErrorKind::UnsafePath, "相对路径组件"))
                }
                #[cfg(windows)]
                Component::Prefix(p)
                    if !matches!(
                        p.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ) =>
                {
                    return Err(error(ErrorKind::UnsafePath, "禁止网络、设备和非磁盘路径"))
                }
                _ => {}
            }
        }
        let mut scan = Self {
            entries_left: MAX_ENTRIES,
            hash_left: MAX_HASH_BYTES,
            pins: Vec::new(),
            snapshots: Vec::new(),
            missing: Vec::new(),
        };
        let mut path = PathBuf::new();
        for part in base.components() {
            path.push(part.as_os_str());
            if matches!(part, Component::Prefix(_)) {
                continue;
            }
            if !scan.pin_dir(&path)? {
                return Err(error(ErrorKind::UnsafePath, "base 祖先不存在"));
            }
        }
        Ok(scan)
    }
    fn pin_dir(&mut self, path: &Path) -> Result<bool> {
        if self.pins.len() + self.missing.len() >= MAX_DIRECTORIES {
            return Err(error(ErrorKind::LimitExceeded, "目录数量超限"));
        }
        if let Some(pin) = Pin::open(path, true)? {
            self.pins.push(pin);
            Ok(true)
        } else {
            self.missing.push(path.into());
            Ok(false)
        }
    }
    fn entries(&mut self, path: &Path) -> Result<Vec<Entry>> {
        let entries = read_entries(path, &mut self.entries_left)?;
        self.snapshots.push(Snapshot {
            path: path.into(),
            entries: entries.clone(),
        });
        Ok(entries)
    }
    fn verify(&mut self) -> Result<()> {
        for pin in &self.pins {
            pin.verify()?;
        }
        for missing in &self.missing {
            match fs::symlink_metadata(missing) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(error(ErrorKind::Changed, "缺失目录状态发生变化")),
            }
        }
        // 检测扫描期间新增/删除、大小或时间变化；上限包括第二遍复核，不取截断结果。
        for snapshot in &self.snapshots {
            if read_entries(&snapshot.path, &mut self.entries_left)? != snapshot.entries {
                return Err(error(ErrorKind::Changed, "候选目录在扫描期间变化"));
            }
        }
        Ok(())
    }
}
fn read_entries(path: &Path, left: &mut usize) -> Result<Vec<Entry>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(path).map_err(|_| error(ErrorKind::Io, "无法枚举目录"))? {
        *left = left
            .checked_sub(1)
            .ok_or_else(|| error(ErrorKind::LimitExceeded, "扫描条目超限"))?;
        let entry = entry.map_err(|_| error(ErrorKind::Io, "目录枚举失败"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| error(ErrorKind::UnsafePath, "文件名编码无效"))?;
        safe_name(&name)?;
        let meta = fs::symlink_metadata(entry.path())
            .map_err(|_| error(ErrorKind::Changed, "枚举期间路径变化"))?;
        result.push(Entry {
            name,
            stamp: stamp(&meta)?,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

fn candidate(
    path: &Path,
    entry: &Entry,
    meta: &AttachmentContent,
    out: &mut Vec<Pin>,
) -> Result<()> {
    if entry.stamp.directory {
        return Err(error(ErrorKind::UnsafePath, "候选附件不是常规文件"));
    }
    let pin = Pin::open(path, false)?.ok_or_else(|| error(ErrorKind::Changed, "候选消失"))?;
    if pin.stamp != entry.stamp {
        return Err(error(ErrorKind::Changed, "候选在打开前变化"));
    }
    if meta
        .expected_size
        .is_some_and(|size| size != pin.stamp.size)
    {
        return Ok(());
    }
    if out.len() >= MAX_CANDIDATES {
        return Err(error(ErrorKind::LimitExceeded, "候选数量超限"));
    }
    out.push(pin);
    Ok(())
}
fn walk_files(
    scan: &mut Scan,
    path: &Path,
    depth: usize,
    meta: &AttachmentContent,
    out: &mut Vec<Pin>,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(error(ErrorKind::LimitExceeded, "目录深度超限"));
    }
    for entry in scan.entries(path)? {
        let child = path.join(&entry.name);
        if !entry.name.starts_with('.') && file_match(&entry.name, &meta.title) {
            candidate(&child, &entry, meta, out)?;
        } else if entry.stamp.directory {
            if !scan.pin_dir(&child)? {
                return Err(error(ErrorKind::Changed, "扫描子目录消失"));
            }
            walk_files(scan, &child, depth + 1, meta, out)?;
        }
    }
    Ok(())
}
fn record_candidates(
    scan: &mut Scan,
    root: &Path,
    meta: &AttachmentContent,
    out: &mut Vec<Pin>,
) -> Result<()> {
    let index = meta
        .item_index
        .ok_or_else(|| error(ErrorKind::InvalidIndex, "记录缺少 item_index"))?;
    for month in scan.entries(root)? {
        if !month.stamp.directory {
            continue;
        }
        let month_path = root.join(&month.name);
        if !scan.pin_dir(&month_path)? {
            return Err(error(ErrorKind::Changed, "月份目录消失"));
        }
        let rec = record_collection(&month_path);
        if !scan.pin_dir(&rec)? {
            continue;
        }
        for card in scan.entries(&rec)? {
            if !card.stamp.directory {
                continue;
            }
            let card_path = rec.join(card.name);
            if !scan.pin_dir(&card_path)? {
                return Err(error(ErrorKind::Changed, "记录目录消失"));
            }
            let Some(media) = record_media(&card_path, meta.kind) else {
                continue;
            };
            if !scan.pin_dir(&media)? {
                continue;
            }
            let media = if meta.kind == Kind::Image {
                media
            } else {
                let path = record_item_directory(&media, meta.kind, index);
                if !scan.pin_dir(&path)? {
                    continue;
                }
                path
            };
            for entry in scan.entries(&media)? {
                let matches = if meta.kind == Kind::Image {
                    image_match(&entry.name, index)
                } else if !meta.title.is_empty() {
                    entry.name == meta.title
                } else {
                    meta.expected_size.is_some()
                };
                if matches {
                    candidate(&media.join(&entry.name), &entry, meta, out)?;
                }
            }
        }
    }
    Ok(())
}
fn md5_reader(reader: &mut impl Read, left: &mut u64) -> Result<(String, u64)> {
    let mut digest = md5::Context::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0;
    loop {
        // 多读至多一个探针字节，增长中的文件不能绕过最初 stat 和累计 500MiB 上限。
        let want = ((*left).saturating_add(1)).min(buffer.len() as u64) as usize;
        let n = reader
            .read(&mut buffer[..want])
            .map_err(|_| error(ErrorKind::Io, "读取候选失败"))?;
        if n == 0 {
            break;
        }
        *left = left
            .checked_sub(n as u64)
            .ok_or_else(|| error(ErrorKind::LimitExceeded, "累计 MD5 读取超过 500MiB"))?;
        total += n as u64;
        digest.consume(&buffer[..n]);
    }
    Ok((format!("{:x}", digest.compute()), total))
}

/// base 必须由调用者绑定到同一账号；本函数不证明账号来源。文本/metadata-only 不访问磁盘。
/// None 表示没有本地引用。无 hash 的唯一候选仍为弱绑定，即使已计算实际 MD5。
pub fn find_reference(base: &Path, meta: &AttachmentMetadata) -> Result<Option<FileReference>> {
    let expected = validate_metadata(meta)?;
    if matches!(meta.kind, Kind::Text | Kind::MetadataOnly) {
        return Ok(None);
    }
    if meta.item_index.is_none() && (meta.kind != Kind::File || meta.title.is_empty()) {
        return Err(error(ErrorKind::InvalidMetadata, "外层文件元数据不完整"));
    }
    let mut scan = Scan::new(base)?;
    let ancestors = cache_ancestors(base, meta);
    for path in &ancestors {
        if !scan.pin_dir(path)? {
            return Ok(None);
        }
    }
    let root = ancestors.last().expect("adapter supplies cache ancestors");
    let mut candidates = Vec::new();
    if meta.item_index.is_some() {
        record_candidates(&mut scan, root, meta, &mut candidates)?;
    } else {
        walk_files(&mut scan, root, 0, meta, &mut candidates)?;
    }
    if candidates.is_empty() {
        scan.verify()?;
        return Ok(None);
    }
    if expected.is_none() && candidates.len() > 1 {
        return Err(error(
            ErrorKind::Ambiguous,
            "多个候选且缺少消息 MD5，不能按时间或路径任取",
        ));
    }
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    let mut matched = Vec::new();
    for pin in candidates {
        if pin.stamp.size > scan.hash_left {
            return Err(error(ErrorKind::LimitExceeded, "累计 MD5 读取超限"));
        }
        let (md5, size) = md5_reader(&mut &pin.file, &mut scan.hash_left)?;
        pin.verify()?;
        if size != pin.stamp.size {
            return Err(error(ErrorKind::Changed, "读取期间文件增长或缩短"));
        }
        if expected.as_ref().is_none_or(|expected| expected == &md5) {
            matched.push((pin, md5, size));
        }
    }
    scan.verify()?;
    let count = matched.len();
    let (mut file, md5, size) = matched
        .into_iter()
        .next()
        .ok_or_else(|| error(ErrorKind::HashMismatch, "候选与消息 MD5 均不匹配"))?;
    file.file
        .rewind()
        .map_err(|_| error(ErrorKind::Io, "重置只读句柄失败"))?;
    Ok(Some(FileReference {
        path: file.path.clone(), size, md5,
        binding: if expected.is_some() { Binding::Md5 } else { Binding::Heuristic },
        warning: expected.is_none().then_some("消息无 MD5；单候选仅按位置/文件名及已知大小弱绑定，可能是其他消息的同名副本，须人工核验。"),
        equivalent_copies: count, file, _directories: scan.pins,
    }))
}

#[cfg(test)]
#[path = "../../tests/fixtures/attachment-refs/tests.rs"]
mod tests;
