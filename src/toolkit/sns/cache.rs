//! SNS 本地缓存恢复，可独立编译；不读取配置/密钥文件，不访问网络或调用 Node/WASM。
//! 图片匹配沿用旧版启发式评分，不能视作媒体 ID 的确定关联。
//! 主调用链：build_cache_index -> recover_post_media -> apply_media_references。
use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroize;

const V1: &[u8; 6] = b"\x07\x08V1\x08\x07";
const V2: &[u8; 6] = b"\x07\x08V2\x08\x07";
const V1_KEY: &[u8; 16] = b"cfcd208495d565ef";
const WINDOW: f64 = 72.0 * 3600.0;

/// 两种缓存根目录必须由调用者按账号显式指定；None 不搜索任何默认位置。
#[derive(Clone, Debug, Default)]
pub struct CacheRoots {
    pub xwechat: Option<PathBuf>,
    pub file_storage_sns: Option<PathBuf>,
}

/// V2 图片密钥仅由调用者显式传入；不实现 Debug，避免日志输出密钥。
pub struct CacheKeys {
    pub image_aes_key: Option<[u8; 16]>,
    pub image_xor_key: u8,
}
impl Default for CacheKeys {
    fn default() -> Self {
        Self {
            image_aes_key: None,
            image_xor_key: 0x88,
        }
    }
}
impl Drop for CacheKeys {
    fn drop(&mut self) {
        self.image_aes_key.zeroize();
        self.image_xor_key.zeroize();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CacheLimits {
    pub max_image_bytes: u64,
    pub max_video_bytes: u64,
    pub max_entries: usize,
}
impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: 256 * 1024 * 1024,
            max_video_bytes: 2 * 1024 * 1024 * 1024,
            max_entries: 200_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub mtime: f64,
    pub estimated_size: u64,
    pub format: String,
    pub width: u32,
    pub height: u32,
    source_size: u64,
    modified: SystemTime,
}

#[derive(Clone, Debug)]
pub struct VideoEntry {
    pub path: PathBuf,
    pub complete: bool,
    source_size: u64,
    modified: SystemTime,
}

#[derive(Debug)]
pub struct CacheIndex {
    images: Vec<ImageEntry>,
    videos: BTreeMap<String, VideoEntry>,
    roots: Vec<PathBuf>,
    limits: CacheLimits,
    pub scanned: usize,
    pub warnings: Vec<String>,
}
impl CacheIndex {
    pub fn images(&self) -> &[ImageEntry] {
        &self.images
    }
    pub fn videos(&self) -> &BTreeMap<String, VideoEntry> {
        &self.videos
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CacheMedia {
    pub media_type: String,
    pub id: String,
    pub width: i64,
    pub height: i64,
    pub total_size: i64,
}
impl CacheMedia {
    /// 同时接受旧 timeline 的字符串字段和 album/feed 的整数类型字段。
    pub fn from_json(value: &Value) -> Result<Self> {
        if !value.is_object() {
            bail!("media must be an object");
        }
        Ok(Self {
            media_type: scalar(&value["type"]),
            id: scalar(&value["id"]),
            width: integer(&value["width"])?,
            height: integer(&value["height"])?,
            total_size: integer(&value["total_size"])?,
        })
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn integer(value: &Value) -> Result<i64> {
    match value {
        Value::Null => Ok(0),
        Value::Number(n) => n.as_i64().ok_or_else(|| anyhow!("integer out of range")),
        Value::String(s) if s.is_empty() => Ok(0),
        Value::String(s) => s.trim().parse().context("invalid integer field"),
        _ => bail!("invalid integer field type"),
    }
}

/// 返回旧实现使用的格式名；这只是文件头识别，不是完整图片解码验证。
pub fn detect_image_format(data: &[u8]) -> &'static str {
    if data.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if data.starts_with(b"\x89PNG") {
        "png"
    } else if data.starts_with(b"GIF") {
        "gif"
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        "webp"
    } else {
        "bin"
    }
}

/// 忠实移植 PNG/JPEG/VP8 尺寸读取；旧代码未支持的 GIF/VP8L/VP8X 返回 (0,0)。
pub fn image_dimensions(data: &[u8]) -> (u32, u32) {
    if data.len() < 24 {
        return (0, 0);
    }
    if data.starts_with(b"\x89PNG") {
        return (
            u32::from_be_bytes(data[16..20].try_into().unwrap()),
            u32::from_be_bytes(data[20..24].try_into().unwrap()),
        );
    }
    if data.starts_with(b"\xff\xd8") {
        let mut i = 2;
        while i < data.len() - 9 {
            if data[i] != 0xff {
                break;
            }
            let marker = data[i + 1];
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                return (
                    u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32,
                    u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32,
                );
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + len;
        }
    }
    if data.len() >= 30
        && data.starts_with(b"RIFF")
        && &data[8..12] == b"WEBP"
        && &data[12..16] == b"VP8 "
    {
        return (
            (u16::from_le_bytes([data[26], data[27]]) & 0x3fff) as u32,
            (u16::from_le_bytes([data[28], data[29]]) & 0x3fff) as u32,
        );
    }
    (0, 0)
}

fn xor_key(data: &[u8]) -> Option<u8> {
    for magic in [b"\xff\xd8\xff".as_slice(), b"\x89PNG", b"GIF8", b"RIFF"] {
        if data.len() >= magic.len() {
            let key = data[0] ^ magic[0];
            if data
                .iter()
                .zip(magic)
                .all(|(byte, expected)| byte ^ key == *expected)
            {
                return Some(key);
            }
        }
    }
    None
}

fn dat_layout(data: &[u8], file_size: u64) -> Result<(usize, u64, u64)> {
    if data.len() < 15 {
        bail!("truncated DAT header");
    }
    let aes_size = u32::from_le_bytes(data[6..10].try_into().unwrap()) as u64;
    let xor_size = u32::from_le_bytes(data[10..14].try_into().unwrap()) as u64;
    // 即使原始 AES 长度已经对齐，也必须再读取一个完整 padding 块。
    let aligned = aes_size + 16 - aes_size % 16;
    if 15 + aligned + xor_size > file_size {
        bail!("DAT segments overlap or are truncated");
    }
    Ok((usize::try_from(aligned)?, aes_size, xor_size))
}

fn aes_key<'a>(data: &[u8], keys: &'a CacheKeys) -> Result<&'a [u8; 16]> {
    if data.starts_with(V1) {
        Ok(V1_KEY)
    } else {
        keys.image_aes_key
            .as_ref()
            .ok_or_else(|| anyhow!("V2 image key not supplied"))
    }
}

fn decrypt_blocks(data: &mut [u8], key: &[u8; 16]) {
    let aes = aes::Aes128::new(GenericArray::from_slice(key));
    for block in data.chunks_exact_mut(16) {
        aes.decrypt_block(GenericArray::from_mut_slice(block));
    }
}

/// 纯内存 DAT 解密：V1/V2 AES-ECB + PKCS#7 + 明文 + XOR，或旧全文件 XOR。
/// 不回显密钥；拒绝旧实现可能容忍的分段重叠/长度矛盾。
pub fn decrypt_dat(data: &[u8], keys: &CacheKeys, max_bytes: u64) -> Result<Vec<u8>> {
    if data.len() as u64 > max_bytes {
        bail!("image exceeds size limit");
    }
    if data.len() < 15 {
        bail!("truncated DAT file");
    }
    if data.starts_with(V1) || data.starts_with(V2) {
        let (aligned, aes_size, xor_size) = dat_layout(data, data.len() as u64)?;
        let mut plain = data[15..15 + aligned].to_vec();
        decrypt_blocks(&mut plain, aes_key(data, keys)?);
        let padding = *plain.last().unwrap() as usize;
        if !(1..=16).contains(&padding)
            || !plain[plain.len() - padding..]
                .iter()
                .all(|p| *p as usize == padding)
        {
            bail!("invalid DAT padding or image key");
        }
        plain.truncate(plain.len() - padding);
        if plain.len() as u64 != aes_size {
            bail!("DAT plaintext length disagrees with header");
        }
        let tail = data.len() - xor_size as usize;
        plain.extend_from_slice(&data[15 + aligned..tail]);
        plain.extend(data[tail..].iter().map(|byte| byte ^ keys.image_xor_key));
        Ok(plain)
    } else {
        let key = xor_key(data).ok_or_else(|| anyhow!("unrecognized DAT format"))?;
        Ok(data.iter().map(|byte| byte ^ key).collect())
    }
}

fn inspect_image(data: &[u8], size: u64, keys: &CacheKeys) -> Result<(u64, String, u32, u32)> {
    if data.len() < 15 {
        bail!("truncated cache header");
    }
    let (plain, estimate) = if data.starts_with(V1) || data.starts_with(V2) {
        let (aligned, aes_size, _) = dat_layout(data, size)?;
        let available = aligned.min(data.len() - 15) / 16 * 16;
        if available == 0 {
            bail!("missing AES header block");
        }
        let mut plain = data[15..15 + available].to_vec();
        decrypt_blocks(&mut plain, aes_key(data, keys)?);
        (plain, size - 15 - (aligned as u64 - aes_size))
    } else {
        let key = xor_key(data).ok_or_else(|| anyhow!("unrecognized cache header"))?;
        (data.iter().map(|byte| byte ^ key).collect(), size)
    };
    let format = detect_image_format(&plain).to_string();
    if format == "bin" {
        bail!("unknown decrypted image header");
    }
    let (width, height) = image_dimensions(&plain);
    Ok((estimate, format, width, height))
}

fn reject_network_path(path: &Path) -> Result<()> {
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = path.components().next() {
        if matches!(
            prefix.kind(),
            std::path::Prefix::UNC(..)
                | std::path::Prefix::VerbatimUNC(..)
                | std::path::Prefix::DeviceNS(..)
        ) {
            bail!("only local filesystem paths are supported");
        }
    }
    let _ = path;
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn children(path: &Path, directory: bool, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if is_link(&meta) || !meta.is_dir() {
        return Vec::new();
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            warnings.push("cache directory could not be read".into());
            return Vec::new();
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            warnings.push("cache entry could not be listed".into());
            continue;
        };
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            warnings.push("cache entry disappeared".into());
            continue;
        };
        if is_link(&meta) {
            warnings.push("cache symlink/reparse entry skipped".into());
            continue;
        }
        if (directory && meta.is_dir()) || (!directory && meta.is_file()) {
            paths.push(path);
        }
    }
    // 保留文件系统枚举顺序；旧实现完全同分时也采用首次出现者。
    paths
}

fn unix_time(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

fn canonical_root(path: &Path) -> Result<PathBuf> {
    reject_network_path(path)?;
    let root = fs::canonicalize(path).context("explicit cache root is unavailable")?;
    if !root.is_dir() {
        bail!("cache root is not a directory");
    }
    Ok(root)
}

fn add_image(index: &mut CacheIndex, path: PathBuf, keys: &CacheKeys) -> Result<()> {
    index.scanned += 1;
    if index.scanned > index.limits.max_entries {
        bail!("cache entry limit exceeded");
    }
    let result = (|| -> Result<ImageEntry> {
        let mut file = checked_source(&path, &index.roots)?;
        let metadata = file.metadata()?;
        if metadata.len() > index.limits.max_image_bytes {
            bail!("image exceeds size limit");
        }
        let mut header = Vec::new();
        (&mut file).take(4096).read_to_end(&mut header)?;
        let (estimated_size, format, width, height) = inspect_image(&header, metadata.len(), keys)?;
        let modified = metadata.modified()?;
        Ok(ImageEntry {
            path,
            mtime: unix_time(modified),
            estimated_size,
            format,
            width,
            height,
            source_size: metadata.len(),
            modified,
        })
    })();
    match result {
        Ok(entry) => index.images.push(entry),
        Err(error) => index
            .warnings
            .push(format!("image cache entry skipped: {error}")),
    }
    Ok(())
}

fn add_video(index: &mut CacheIndex, path: PathBuf) -> Result<()> {
    index.scanned += 1;
    if index.scanned > index.limits.max_entries {
        bail!("cache entry limit exceeded");
    }
    let key = format!(
        "{}{}",
        path.parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy(),
        path.file_stem().unwrap_or_default().to_string_lossy()
    )
    .to_ascii_lowercase();
    if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    let result = (|| -> Result<VideoEntry> {
        let file = checked_source(&path, &index.roots)?;
        let metadata = file.metadata()?;
        if metadata.len() > index.limits.max_video_bytes {
            bail!("video exceeds size limit");
        }
        let complete = path
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("mp4"));
        Ok(VideoEntry {
            path,
            complete,
            source_size: metadata.len(),
            modified: metadata.modified()?,
        })
    })();
    match result {
        Ok(entry) => {
            // 忠实保留旧 OR 规则：较大的非 mp4 候选也可能替代较小的 mp4。
            let replace = index
                .videos
                .get(&key)
                .is_none_or(|prev| entry.complete || entry.source_size > prev.source_size);
            if replace {
                index.videos.insert(key, entry);
            }
        }
        Err(error) => index
            .warnings
            .push(format!("video cache entry skipped: {error}")),
    }
    Ok(())
}

/// 只扫描两个旧布局：月份/Sns/Img/分片/文件、月份/文件；另索引月份/Sns/Video/分片/文件。
/// 缺失的显式根目录返回错误，不扩大搜索到其他账号或默认目录。
pub fn build_cache_index(
    roots: &CacheRoots,
    keys: &CacheKeys,
    limits: CacheLimits,
) -> Result<CacheIndex> {
    build_index(roots, Some(keys), limits)
}

/// 只索引显式 xwechat 根下的视频缓存，不遍历或解密图片，不需要图片密钥。
/// 根目录、扫描限额及视频候选规则与 build_cache_index 一致。
pub fn build_video_cache_index(root: &Path, limits: CacheLimits) -> Result<CacheIndex> {
    build_index(
        &CacheRoots {
            xwechat: Some(root.to_path_buf()),
            file_storage_sns: None,
        },
        None,
        limits,
    )
}

fn build_index(
    roots: &CacheRoots,
    keys: Option<&CacheKeys>,
    limits: CacheLimits,
) -> Result<CacheIndex> {
    let xwechat = roots.xwechat.as_deref().map(canonical_root).transpose()?;
    let legacy = if keys.is_some() {
        roots
            .file_storage_sns
            .as_deref()
            .map(canonical_root)
            .transpose()?
    } else {
        None
    };
    let mut index = CacheIndex {
        roots: xwechat.iter().chain(legacy.iter()).cloned().collect(),
        images: Vec::new(),
        videos: BTreeMap::new(),
        limits,
        scanned: 0,
        warnings: Vec::new(),
    };
    if let Some(root) = xwechat {
        for month in children(&root, true, &mut index.warnings) {
            let sns = month.join("Sns");
            if fs::symlink_metadata(&sns).is_ok_and(|m| is_link(&m)) {
                index.warnings.push("Sns reparse directory skipped".into());
                continue;
            }
            if let Some(keys) = keys {
                for shard in children(&sns.join("Img"), true, &mut index.warnings) {
                    for path in children(&shard, false, &mut index.warnings) {
                        add_image(&mut index, path, keys)?;
                    }
                }
            }
            for shard in children(&sns.join("Video"), true, &mut index.warnings) {
                for path in children(&shard, false, &mut index.warnings) {
                    add_video(&mut index, path)?;
                }
            }
        }
    }
    if let (Some(root), Some(keys)) = (legacy, keys) {
        for month in children(&root, true, &mut index.warnings) {
            for path in children(&month, false, &mut index.warnings) {
                if path
                    .file_name()
                    .is_some_and(|p| p.to_string_lossy().ends_with("_t"))
                {
                    continue;
                }
                add_image(&mut index, path, keys)?;
            }
        }
    }
    index.images.sort_by(|a, b| a.mtime.total_cmp(&b.mtime));
    Ok(index)
}

/// 返回与媒体列表等长的索引位置；未匹配/非图片为 None。时窗为空时按旧实现扩大到全索引。
pub fn match_cache_images(
    index: &CacheIndex,
    create_time: i64,
    media: &[CacheMedia],
) -> Vec<Option<usize>> {
    let time = create_time as f64;
    let mut lo = index
        .images
        .partition_point(|entry| entry.mtime < time - WINDOW);
    let mut hi = index
        .images
        .partition_point(|entry| entry.mtime <= time + WINDOW);
    if lo >= hi {
        lo = 0;
        hi = index.images.len();
    }
    let mut used = BTreeSet::new();
    media
        .iter()
        .map(|media| {
            if !matches!(media.media_type.as_str(), "2" | "") {
                return None;
            }
            let mut best: Option<(usize, u64, f64)> = None;
            for (i, entry) in index.images.iter().enumerate().take(hi).skip(lo) {
                if used.contains(&entry.path) {
                    continue;
                }
                if media.width > 0
                    && media.height > 0
                    && entry.width > 0
                    && entry.height > 0
                    && (media.width != entry.width as i64 || media.height != entry.height as i64)
                {
                    continue;
                }
                if media.total_size > 0
                    && (entry.estimated_size as f64 > media.total_size as f64 * 3.0
                        || (entry.estimated_size as f64) < media.total_size as f64 * 0.3)
                {
                    continue;
                }
                let size_diff = if media.total_size > 0 {
                    entry.estimated_size.abs_diff(media.total_size as u64)
                } else {
                    0
                };
                let time_diff = (entry.mtime - time).abs();
                if best.is_none()
                    || best.is_some_and(|(_, size, time)| {
                        size_diff < size || (size_diff == size && time_diff < time)
                    })
                {
                    best = Some((i, size_diff, time_diff));
                }
            }
            best.map(|(i, _, _)| {
                used.insert(index.images[i].path.clone());
                i
            })
        })
        .collect()
}

pub fn video_cache_key(post_id: &str, media_id: &str) -> String {
    format!(
        "{:x}",
        md5::compute(format!("{}_{}_3", post_id.trim(), media_id.trim()).as_bytes())
    )
}

/// 保持旧优先级：XML/post_id、数据库 tid 原值、tid 按 u64 重解释。
pub fn find_cached_video<'a>(
    index: &'a CacheIndex,
    post_id: &str,
    tid: Option<i64>,
    media_id: &str,
) -> Option<&'a VideoEntry> {
    if media_id.trim().is_empty() {
        return None;
    }
    let mut ids = vec![post_id.trim().to_string()];
    if let Some(tid) = tid {
        if tid != 0 {
            ids.push(tid.to_string());
        }
        ids.push((tid as u64).to_string());
    }
    ids.iter()
        .filter(|id| !id.is_empty())
        .find_map(|id| index.videos.get(&video_cache_key(id, media_id)))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecoveryOptions {
    /// partial 标志仅沿用旧文件扩展名规则，不是完整 MP4 结构验证。
    pub allow_partial_video: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaRecovery {
    pub media_index: usize,
    pub status: String,
    /// 与旧相册字段兼容的 JSON 补丁，保留 bool/整数类型，不强塞回字符串映射。
    pub reference: Option<Value>,
    pub match_method: Option<String>,
    pub bytes: u64,
}

#[derive(Clone, Serialize)]
pub struct RecoveryReport {
    pub media: Vec<MediaRecovery>,
    pub warnings: Vec<String>,
    // 完整快照仅留内存，用来防止把结果附到另一条/已重新排序的动态；不写入日志。
    #[serde(skip)]
    original_post: Value,
}

fn checked_source(path: &Path, roots: &[PathBuf]) -> Result<File> {
    reject_network_path(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) || !metadata.is_file() {
        bail!("cache source is not a regular file");
    }
    let canonical = fs::canonicalize(path)?;
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        bail!("cache source escapes explicit roots");
    }
    File::open(canonical).context("cache source unavailable")
}

fn verify_snapshot(file: &File, size: u64, modified: SystemTime) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.len() != size || metadata.modified()? != modified {
        bail!("cache changed since indexing; rebuild index");
    }
    Ok(())
}

fn output_directory(output: &Path, subdir: &str, index: &CacheIndex) -> Result<PathBuf> {
    reject_network_path(output)?;
    if output
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        bail!("output must not contain parent traversal");
    }
    let mut ancestor = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    let mut missing = Vec::new();
    while fs::symlink_metadata(&ancestor).is_err() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| anyhow!("invalid output path"))?
                .to_os_string(),
        );
        if !ancestor.pop() {
            bail!("no output ancestor exists");
        }
    }
    let mut root = fs::canonicalize(ancestor)?;
    for part in missing.into_iter().rev() {
        root.push(part);
    }
    if index.roots.iter().any(|cache| root.starts_with(cache)) {
        bail!("output must not be inside cache roots");
    }
    let path = root.join(subdir);
    if index.roots.iter().any(|cache| path.starts_with(cache)) {
        bail!("media output must not be inside cache roots");
    }
    fs::create_dir_all(&root)?;
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if is_link(&meta) || !meta.is_dir() {
            bail!("media output directory is unsafe");
        }
    } else {
        fs::create_dir(&path)?;
    }
    Ok(path)
}

fn write_new(path: &Path, write: impl FnOnce(&mut File) -> Result<u64>) -> Result<u64> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("media destination already exists or is unavailable")?;
    let result = write(&mut file).and_then(|count| {
        file.sync_all()?;
        Ok(count)
    });
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn restore_image(
    index: &CacheIndex,
    entry: &ImageEntry,
    keys: &CacheKeys,
    output: &Path,
    stem: &str,
    media_index: usize,
) -> Result<(Value, u64)> {
    let mut source = checked_source(&entry.path, &index.roots)?;
    verify_snapshot(&source, entry.source_size, entry.modified)?;
    let mut data = Vec::new();
    (&mut source)
        .take(index.limits.max_image_bytes.saturating_add(1))
        .read_to_end(&mut data)?;
    verify_snapshot(&source, entry.source_size, entry.modified)?;
    let plain = decrypt_dat(&data, keys, index.limits.max_image_bytes)?;
    let format = detect_image_format(&plain);
    if format == "bin" {
        bail!("unrecognized recovered image header");
    }
    if format != entry.format {
        bail!("recovered image format differs from indexed header");
    }
    let dir = output_directory(output, "images", index)?;
    let name = format!("{stem}_{media_index}.{format}");
    let bytes = write_new(&dir.join(&name), |target| {
        target.write_all(&plain)?;
        Ok(plain.len() as u64)
    })?;
    Ok((
        json!({"local_file": format!("images/{name}"), "image_source": "cache"}),
        bytes,
    ))
}

fn restore_video(
    index: &CacheIndex,
    entry: &VideoEntry,
    output: &Path,
    stem: &str,
    media_index: usize,
) -> Result<(Value, u64)> {
    let mut source = checked_source(&entry.path, &index.roots)?;
    verify_snapshot(&source, entry.source_size, entry.modified)?;
    let mut header = [0; 16];
    let count = source.read(&mut header)?;
    if count < 12 || &header[4..8] != b"ftyp" {
        bail!("cached video has no MP4 header");
    }
    if entry.source_size > index.limits.max_video_bytes {
        bail!("video exceeds size limit");
    }
    let dir = output_directory(output, "videos", index)?;
    let name = format!("{stem}_{media_index}.mp4");
    let bytes = write_new(&dir.join(&name), |target| {
        target.write_all(&header[..count])?;
        let remaining = index
            .limits
            .max_video_bytes
            .saturating_sub(count as u64)
            .saturating_add(1);
        let copied = std::io::copy(&mut (&mut source).take(remaining), target)? + count as u64;
        if copied > index.limits.max_video_bytes || copied != entry.source_size {
            bail!("cached video changed or exceeds limit");
        }
        verify_snapshot(&source, entry.source_size, entry.modified)?;
        // 与 shutil.copy2 的主要时间语义一致，保留缓存修改时间。
        target.set_times(fs::FileTimes::new().set_modified(entry.modified))?;
        Ok(copied)
    })?;
    Ok((
        json!({"local_file": format!("videos/{name}"), "video_source": "cache", "video_complete": entry.complete}),
        bytes,
    ))
}

/// 不修改输入 JSON/现有导出文件。stem 由调用者传入唯一动态文件名（不含扩展名）。
/// 单项失败进入报告，后续媒体继续；仅顶层参数/帖子结构错误返回 Err。
pub fn recover_post_media(
    index: &CacheIndex,
    post: &Value,
    output: &Path,
    stem: &str,
    keys: &CacheKeys,
    options: RecoveryOptions,
) -> Result<RecoveryReport> {
    if stem.is_empty()
        || stem.len() > 120
        || !stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("unsafe media filename stem");
    }
    let media = post
        .get("media")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("post.media must be an array"))?;
    let create_time = integer(&post["create_time"])?;
    let tid = match &post["tid"] {
        Value::Null => None,
        value => Some(integer(value)?),
    };
    let post_id = if scalar(&post["post_id"]).is_empty() {
        scalar(&post["id"])
    } else {
        scalar(&post["post_id"])
    };
    let parsed: Vec<_> = media.iter().map(CacheMedia::from_json).collect();
    let requests: Vec<_> = parsed
        .iter()
        .map(|p| {
            p.as_ref().cloned().unwrap_or_else(|_| CacheMedia {
                media_type: "invalid".into(),
                ..Default::default()
            })
        })
        .collect();
    let matches = match_cache_images(index, create_time, &requests);
    let mut report = RecoveryReport {
        media: Vec::new(),
        warnings: Vec::new(),
        original_post: post.clone(),
    };
    for (i, request) in requests.iter().enumerate() {
        let mut item = MediaRecovery {
            media_index: i,
            status: "missing".into(),
            reference: None,
            match_method: None,
            bytes: 0,
        };
        let result = if parsed[i].is_err() {
            item.status = "invalid_metadata".into();
            None
        } else if media[i]
            .get("local_file")
            .is_some_and(|v| !v.is_null() && v != "")
        {
            // 不覆盖调用者已有引用；存在不等于已验证，报告中不标为恢复成功。
            item.status = "existing_reference".into();
            None
        } else if matches!(request.media_type.as_str(), "" | "2") {
            matches[i].map(|entry| {
                item.match_method = Some("legacy_image_heuristic".into());
                restore_image(index, &index.images[entry], keys, output, stem, i)
            })
        } else if matches!(request.media_type.as_str(), "6" | "15") {
            if request.id.trim().is_empty() {
                item.status = "missing_media_id".into();
                None
            } else if let Some(entry) = find_cached_video(index, &post_id, tid, &request.id) {
                item.match_method = Some("post_media_md5".into());
                if !entry.complete && !options.allow_partial_video {
                    item.status = "partial_video_disabled".into();
                    None
                } else {
                    Some(restore_video(index, entry, output, stem, i))
                }
            } else {
                None
            }
        } else {
            item.status = "unsupported_media_type".into();
            None
        };
        if let Some(result) = result {
            match result {
                Ok((reference, bytes)) => {
                    item.status = "recovered".into();
                    item.reference = Some(reference);
                    item.bytes = bytes;
                }
                Err(error) => {
                    item.status = "failed".into();
                    report.warnings.push(format!("media {i}: {error}"));
                }
            }
        }
        report.media.push(item);
    }
    Ok(report)
}

/// 只附加成功恢复项的本地字段，不改 URL/令牌/媒体顺序。拒绝错帖或并发改动后应用。
pub fn apply_media_references(post: &mut Value, report: &RecoveryReport) -> Result<()> {
    if post != &report.original_post {
        bail!("post changed since media recovery");
    }
    let mut updated = post.clone();
    let media = updated
        .get_mut("media")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("post.media must be an array"))?;
    for item in &report.media {
        if let Some(reference) = &item.reference {
            let target = media
                .get_mut(item.media_index)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| anyhow!("media index or object is invalid"))?;
            let fields = reference
                .as_object()
                .ok_or_else(|| anyhow!("invalid local media reference"))?;
            target.extend(fields.clone());
        }
    }
    *post = updated;
    Ok(())
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod cache_tests;
