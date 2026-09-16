//! 严格的本地图片导出；调用者负责验证消息身份及账号与资源根的绑定。
//! 不查询消息库、不获取密钥、不调用网络或外部转换器。仅支持 Windows 本机静态源。

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::decoder;
pub(crate) use super::local_files::HostOutputGuard;
use super::local_files::{safe_name, Pin, Scan};
pub(crate) use crate::adapters::wechat::media::resource::no_sidecars;
pub use crate::adapters::wechat::media::resource::MessageIdentity;
use crate::adapters::wechat::media::resource::{ResourceLookup, ResourceReader};

pub const MAX_DAT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RESOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CANDIDATES: usize = 128;

pub struct ImageRequest<'a> {
    pub message: &'a MessageIdentity,
    pub resource_db: &'a Path,
    /// 显式的 msg/attach 根，不推断配置或账号路径。
    pub attach_root: &'a Path,
    /// 已存在、受调用者信任的目录，必须在源附件树之外。
    pub output_root: &'a Path,
    /// 必须显式传参；V2 的 aes_key=None 会失败，绝不调用 provider。
    pub key: decoder::V2KeyMaterial<'a>,
}

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    /// 0=原图，1=_h，2=_t。相同最优等级多候选拒绝，不任取月份。
    pub rank: u8,
}

#[derive(Debug, Serialize)]
pub struct ImageOutput {
    pub message: MessageIdentity,
    pub path: PathBuf,
    pub source_path: PathBuf,
    pub resource_rowid: i64,
    pub resource_md5: String,
    pub dat_md5: String,
    pub decoded_md5: String,
    pub size: u64,
    pub format: &'static str,
    pub decoder: &'static str,
    pub candidates: Vec<Candidate>,
    /// 资源 packed_info 扫描和文件名只提供关联证据，不证明明文 MD5 或消息真实性。
    pub binding: &'static str,
}

fn resource(path: &Path, identity: &MessageIdentity) -> Result<(i64, String)> {
    let reader = ResourceReader::open(path)?;
    let result = reader.lookup(identity)?;
    no_sidecars(path)?;
    match result {
        ResourceLookup::Found(rowid, md5) => Ok((rowid, md5)),
        ResourceLookup::Missing => anyhow::bail!("exact image resource not found"),
        ResourceLookup::Ambiguous => {
            anyhow::bail!("ambiguous exact image resource; chat mapping must be unique")
        }
        ResourceLookup::Md5Missing => anyhow::bail!("resource MD5 missing"),
    }
}

pub(super) fn scan_candidates(
    scan: &mut Scan,
    chat: &Path,
    hashes: &HashSet<String>,
) -> Result<HashMap<String, Vec<Candidate>>> {
    let mut found: HashMap<String, Vec<Candidate>> = HashMap::new();
    for (month, meta) in scan.entries(chat)? {
        if !meta.0 {
            continue;
        }
        let month = chat.join(month);
        scan.directory(&month)?;
        let img = month.join("Img");
        if !scan.optional_directory(&img)? {
            continue;
        }
        for (name, meta) in scan.entries(&img)? {
            let lower = name.to_ascii_lowercase();
            for (rank, suffix) in [".dat", "_h.dat", "_t.dat"].iter().enumerate() {
                let Some(hash) = lower.strip_suffix(suffix) else {
                    continue;
                };
                if !hashes.contains(hash) {
                    continue;
                }
                ensure!(!meta.0, "DAT candidate is not a regular file");
                let candidates = found.entry(hash.into()).or_default();
                ensure!(
                    candidates.len() < MAX_CANDIDATES,
                    "candidate limit exceeded"
                );
                candidates.push(Candidate {
                    path: img.join(&name),
                    rank: rank as u8,
                });
            }
        }
    }
    for candidates in found.values_mut() {
        candidates.sort_by(|a, b| a.rank.cmp(&b.rank).then(a.path.cmp(&b.path)));
    }
    Ok(found)
}

/// 只读取显式源，复用现有 decoder，并以 persist_noclobber 发布完整文件。
pub fn export_image(request: ImageRequest<'_>) -> Result<ImageOutput> {
    export_image_impl(request, None, None, || Ok(()))
}

/// 保留宿主最初批准的路径身份，跨异步账号查询一直复核到最终发布前。
pub(crate) fn export_image_with_guard(
    request: ImageRequest<'_>,
    guard: &HostOutputGuard,
) -> Result<ImageOutput> {
    ensure!(
        request.output_root == guard.output_root(),
        "host output root mismatch"
    );
    guard.verify()?;
    export_image_impl(request, Some(guard), None, || Ok(()))
}

pub(crate) fn export_image_with_proof(
    request: ImageRequest<'_>,
    guard: &HostOutputGuard,
    resource_proof: (i64, &str),
    before_publish: impl FnOnce() -> Result<()>,
) -> Result<ImageOutput> {
    ensure!(
        request.output_root == guard.output_root(),
        "host output root mismatch"
    );
    guard.verify()?;
    export_image_impl(request, Some(guard), Some(resource_proof), before_publish)
}

fn export_image_impl(
    request: ImageRequest<'_>,
    host_guard: Option<&HostOutputGuard>,
    resource_proof: Option<(i64, &str)>,
    before_publish: impl FnOnce() -> Result<()>,
) -> Result<ImageOutput> {
    let identity = request.message;
    ensure!(
        !identity.username.is_empty()
            && identity.username.len() <= 4096
            && !identity.username.chars().any(char::is_control),
        "invalid username"
    );
    ensure!(
        !identity.source.is_empty()
            && identity.source.len() <= 4096
            && !identity.source.chars().any(char::is_control),
        "invalid logical source"
    );
    ensure!(
        identity.local_id > 0
            && identity.create_time > 0
            && identity.local_type > 0
            && identity.local_type & 0xffffffff == 3,
        "invalid image identity"
    );
    let mut scan = Scan::new();
    scan.root(request.attach_root)?;
    let attach_id = same_file::Handle::from_path(request.attach_root)?;
    let resource_parent = request
        .resource_db
        .parent()
        .context("resource parent missing")?;
    scan.root(resource_parent)?;
    safe_name(
        request
            .resource_db
            .file_name()
            .and_then(|n| n.to_str())
            .context("resource filename missing")?,
    )?;
    let db = Pin::open(request.resource_db, false)?;
    ensure!(
        db.stamp.1 <= MAX_RESOURCE_BYTES,
        "resource database limit exceeded"
    );
    let mut output_guard = Scan::new();
    output_guard.root(request.output_root)?;
    ensure!(
        !output_guard.pins.iter().any(|p| p.id == attach_id),
        "output must be outside attachment source"
    );
    ensure!(
        !same_file::is_same_file(request.output_root, resource_parent)?,
        "output must be separate from resource directory"
    );
    let (resource_rowid, resource_md5) = resource(request.resource_db, identity)?;
    if let Some((rowid, digest)) = resource_proof {
        ensure!(
            rowid == resource_rowid && digest == resource_md5,
            "image resource evidence changed"
        );
    }
    let chat = request
        .attach_root
        .join(format!("{:x}", md5::compute(identity.username.as_bytes())));
    scan.directory(&chat)?;
    let candidates = scan_candidates(&mut scan, &chat, &HashSet::from([resource_md5.clone()]))?
        .remove(&resource_md5)
        .unwrap_or_default();
    let chosen = candidates.first().context("local DAT not found")?;
    ensure!(
        candidates
            .get(1)
            .is_none_or(|other| other.rank != chosen.rank),
        "ambiguous DAT candidates"
    );
    let source = Pin::open(&chosen.path, false)?;
    ensure!(source.stamp.1 <= MAX_DAT_BYTES, "DAT size limit exceeded");
    let mut bytes = Vec::new();
    (&source.file)
        .take(MAX_DAT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == source.stamp.1 && bytes.len() as u64 <= MAX_DAT_BYTES,
        "DAT changed or exceeded limit"
    );
    let decoded = decoder::restore(&bytes, request.key)?;
    let decoded_md5 = format!("{:x}", md5::compute(&decoded.data));
    let path = request
        .output_root
        .join(format!("{decoded_md5}.{}", decoded.format));
    if let Some(guard) = host_guard {
        guard.verify()?;
    }
    let target = crate::infrastructure::publication::ExportTarget::new_file(
        &path,
        &[
            request.attach_root.to_owned(),
            request.resource_db.to_owned(),
            source.path.clone(),
        ],
    )
    .context("output already exists or publication failed")?;
    target
        .write_bytes_checked(&decoded.data, || {
            source.verify()?;
            db.verify()?;
            no_sidecars(request.resource_db)?;
            scan.verify()?;
            output_guard.verify()?;
            if let Some(guard) = host_guard {
                guard.verify()?;
            }
            before_publish()
        })
        .context("output already exists or publication failed")?;
    Ok(ImageOutput {
        message: identity.clone(),
        path,
        source_path: source.path.clone(),
        resource_rowid,
        resource_md5,
        dat_md5: format!("{:x}", md5::compute(&bytes)),
        decoded_md5,
        size: decoded.data.len() as u64,
        format: decoded.format,
        decoder: decoded.decoder,
        candidates,
        binding: "resource_scan_filename_heuristic",
    })
}

#[cfg(test)]
#[path = "../../tests/fixtures/native-image/tests.rs"]
mod tests;
