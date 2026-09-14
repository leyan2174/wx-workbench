//! Bounded local media reads shared by directory and WeChat media adapters.
use crate::attachment::local_files::HostOutputGuard;
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// 明确的扫描预算，不使用 glob 跟随目录联接；守卫活到候选读取结束。
pub(crate) struct Scan {
    left: usize,
    guards: Vec<HostOutputGuard>,
}
impl Scan {
    pub(crate) fn new() -> Self {
        Self {
            left: 20_000,
            guards: Vec::new(),
        }
    }
    pub(crate) fn entries(&mut self, root: &Path) -> Result<Vec<(PathBuf, bool)>> {
        match fs::symlink_metadata(root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        ensure!(self.guards.len() < 1024, "媒体目录数量超过上限");
        self.guards.push(HostOutputGuard::new(root)?);
        let mut entries = Vec::new();
        for entry in fs::read_dir(root)? {
            self.left = self
                .left
                .checked_sub(1)
                .context("媒体目录扫描条目超过上限")?;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            use std::os::windows::fs::MetadataExt;
            ensure!(
                meta.file_attributes() & 0x400 == 0 && !meta.file_type().is_symlink(),
                "媒体目录含重解析路径，拒绝跟随"
            );
            ensure!(meta.is_file() || meta.is_dir(), "媒体目录含非常规文件");
            entries.push((entry.path(), meta.is_dir()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }
    pub(crate) fn verify(&self) -> Result<()> {
        for guard in &self.guards {
            guard.verify()?;
        }
        Ok(())
    }
    pub(crate) fn walk(
        &mut self,
        root: &Path,
        depth: usize,
        accept: &impl Fn(&Path) -> bool,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        ensure!(depth <= 8, "媒体目录扫描深度超过上限");
        for (path, is_dir) in self.entries(root)? {
            if is_dir {
                self.walk(&path, depth + 1, accept, out)?;
            } else if accept(&path) {
                ensure!(out.len() < 128, "媒体候选超过上限");
                out.push(path);
            }
        }
        Ok(())
    }
}
pub(crate) fn bounded_read(path: &Path, stage: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut guard = HostOutputGuard::new(stage)?;
    guard.pin_input(path)?;
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "媒体文件超过单文件或剩余预算"
    );
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "媒体读取超过预算");
    guard.verify()?;
    Ok(bytes)
}
pub(crate) fn hash32(raw: &str) -> Result<String> {
    ensure!(
        raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
        "媒体 MD5 无效或缺失"
    );
    Ok(raw.to_ascii_lowercase())
}
