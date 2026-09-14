//! SNS 输出树预检与逐文件发布，不提供整树事务。
use crate::attachment::local_files::HostOutputGuard;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MANIFEST: &str = "_source_binding.json";
const LOCK: &str = ".wx-sns-publish.lock";
const MAX_DEPTH: usize = 4;
const MAX_MANIFEST: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub version: u32,
    pub tree_kind: String,
    pub source_kind: String,
    pub source_id: String,
    pub user_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExistingPolicy {
    Reject,
    Update,
    Adopt,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    #[serde(flatten)]
    binding: Binding,
    legacy_unverified: bool,
}

pub(crate) struct OutputTree {
    root: PathBuf,
    guards: BTreeMap<PathBuf, HostOutputGuard>,
    targets: BTreeSet<PathBuf>,
    // 保留锁文件名，避免协作写者之间删除后重建锁文件的竞态。
    lock: File,
    manifest: File,
    legacy_unverified: bool,
}

fn relative(path: &Path, directory: bool) -> Result<()> {
    if directory && path.as_os_str().is_empty() {
        return Ok(());
    }
    let raw = path.to_str().context("invalid output path encoding")?;
    ensure!(
        !raw.is_empty() && !path.is_absolute(),
        "relative output path required"
    );
    ensure!(
        !raw.split(['/', '\\'])
            .any(|s| s.is_empty() || s == "." || s == ".."),
        "unsafe relative output path"
    );
    ensure!(
        path.components().count() <= MAX_DEPTH,
        "output path too deep"
    );
    for part in path.components() {
        let Component::Normal(name) = part else {
            anyhow::bail!("unsafe output component");
        };
        let name = name.to_str().context("invalid output component encoding")?;
        ensure!(
            !name.ends_with([' ', '.'])
                && !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
            "unsafe output component"
        );
        let stem = name.split('.').next().unwrap().to_uppercase();
        let numbered = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        ensure!(
            !matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ) && !numbered.is_some_and(|n| matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )),
            "device output component"
        );
    }
    Ok(())
}

fn check_file(file: &File, path: &Path) -> Result<()> {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.file_attributes() & 0x400 == 0,
        "unsafe file type"
    );
    ensure!(
        same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)?,
        "file identity changed"
    );
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)? };
    ensure!(info.nNumberOfLinks == 1, "multiply linked file rejected");
    Ok(())
}

fn read_file(guard: &HostOutputGuard, path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    guard.verify_replaceable_file(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    check_file(&file, path)?;
    Ok(file)
}

/// 旧目录认领只把真正缺失当作无元数据；读取期间固定路径和文件身份。
pub(crate) fn read_legacy_json(path: &Path, limit: u64) -> Result<Option<serde_json::Value>> {
    match fs::symlink_metadata(path) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("旧 SNS 元数据无法读取"),
    }
    let guard = HostOutputGuard::new(path.parent().context("旧 SNS 元数据缺少父目录")?)?;
    read_bounded_json(read_file(&guard, path)?, limit).map(Some)
}

fn read_bounded_json(reader: impl Read, limit: u64) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    // 不依赖初始文件长度，防止读取期间增长突破内存边界。
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "旧 SNS 元数据过大，无法安全认领"
    );
    serde_json::from_slice(&bytes).context("旧 SNS 元数据无法解析")
}

// 先检查并固定最近的已有祖先，再逐级创建缺失目录。
fn make_root(root: &Path, inputs: &[PathBuf]) -> Result<HostOutputGuard> {
    ensure!(root.is_absolute(), "absolute output root required");
    let raw = root.to_str().context("invalid root encoding")?;
    ensure!(
        !raw.split(['/', '\\']).any(|s| s == "." || s == ".."),
        "unsafe output root"
    );
    let mut ancestor = root;
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().context("missing output ancestor")?;
                relative(Path::new(name), false)?;
                missing.push(name.to_owned());
                ancestor = ancestor.parent().context("missing output ancestor")?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let mut guard = HostOutputGuard::new(ancestor)?;
    let mut projected = fs::canonicalize(ancestor)?;
    for name in missing.iter().rev() {
        projected.push(name);
    }
    let projected = PathBuf::from(projected.to_string_lossy().to_lowercase());
    let mut input_guards = Vec::new();
    for input in inputs {
        ensure!(input.is_absolute(), "absolute protected input required");
        let metadata = fs::symlink_metadata(input).context("protected input must exist")?;
        input_guards.push(HostOutputGuard::new(if metadata.is_dir() {
            input
        } else {
            input.parent().context("protected input parent missing")?
        })?);
        let source = PathBuf::from(fs::canonicalize(input)?.to_string_lossy().to_lowercase());
        ensure!(
            !(source.starts_with(&projected)
                || metadata.is_dir() && projected.starts_with(&source)),
            "output overlaps protected input"
        );
    }
    let mut current = ancestor.to_path_buf();
    for name in missing.into_iter().rev() {
        guard.verify()?;
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        guard = HostOutputGuard::new(&current)?;
    }
    Ok(guard)
}

/// 调用者须列出全部候选文件及附属文件；未知旧文件保持不动。
/// 来源标识仅比较调用者传入的值，不据此推断或证明账号归属。
/// 候选数量由调用者的输入资源预算约束，此处不另设媒体数量上限。
pub(crate) fn prepare(
    root: &Path,
    binding: &Binding,
    policy: ExistingPolicy,
    targets: &[PathBuf],
    inputs: &[PathBuf],
    validate_legacy: impl FnOnce(&Path) -> Result<()>,
) -> Result<OutputTree> {
    ensure!(binding.version == 1, "unsupported binding version");
    ensure!(
        [
            &binding.tree_kind,
            &binding.source_kind,
            &binding.source_id,
            &binding.user_name
        ]
        .iter()
        .all(|s| !s.is_empty()),
        "empty source binding"
    );
    let mut planned = BTreeSet::new();
    let mut names = BTreeMap::new();
    let mut parents = BTreeSet::new();
    for path in targets {
        relative(path, false)?;
        let first = path
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .to_lowercase();
        ensure!(first != MANIFEST && first != LOCK, "reserved output target");
        ensure!(planned.insert(path.clone()), "duplicate output target");
        let mut prefix = PathBuf::new();
        for part in path.components() {
            prefix.push(part);
            let key = prefix.to_string_lossy().to_lowercase();
            if let Some(previous) = names.insert(key, prefix.clone()) {
                ensure!(previous == prefix, "case-folded output alias");
            }
        }
        let mut parent = path.parent().unwrap();
        while !parent.as_os_str().is_empty() {
            parents.insert(parent.to_path_buf());
            parent = parent.parent().unwrap();
        }
    }
    if binding.tree_kind == "album" {
        parents.extend([PathBuf::from("images"), PathBuf::from("videos")]);
    }
    ensure!(parents.len() <= 128, "too many output directories");
    ensure!(
        parents.is_disjoint(&planned),
        "target also used as directory"
    );
    let mut root_guard = make_root(root, inputs)?;
    for input in inputs {
        fs::symlink_metadata(input).context("protected input must exist")?;
        root_guard.protect(input)?;
    }
    root_guard.verify_replaceable_file(&root.join(MANIFEST))?;
    root_guard.verify_replaceable_file(&root.join(LOCK))?;
    use std::os::windows::fs::OpenOptionsExt;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(root.join(LOCK))
        .context("SNS output tree is locked or unavailable")?;
    check_file(&lock, &root.join(LOCK))?;
    // 只检查直属条目，不递归扫描未知旧子树。
    let nonempty = fs::read_dir(root)?.try_fold(false, |found, entry| -> Result<bool> {
        Ok(found || entry?.file_name() != LOCK)
    })?;
    // 锁下先拒绝绑定冲突或缺少授权的旧目录，不为拒绝的请求创建媒体子目录。
    let manifest_path = root.join(MANIFEST);
    let existing_manifest = match fs::symlink_metadata(&manifest_path) {
        Ok(_) => {
            let file = read_file(&root_guard, &manifest_path)?;
            let mut bytes = Vec::new();
            (&file).take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_MANIFEST,
                "binding manifest too large"
            );
            let old: Manifest =
                serde_json::from_slice(&bytes).context("invalid source binding manifest")?;
            ensure!(old.binding == *binding, "SNS source binding conflict");
            ensure!(
                policy != ExistingPolicy::Reject,
                "SNS output already exists"
            );
            Some((file, old.legacy_unverified))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                !nonempty || policy == ExistingPolicy::Adopt,
                "unbound nonempty SNS output requires Adopt"
            );
            None
        }
        Err(e) => return Err(e.into()),
    };
    let mut guards = BTreeMap::from([(PathBuf::new(), root_guard)]);
    for parent in parents {
        let ancestor = parent.parent().unwrap();
        guards.get(ancestor).context("unplanned parent")?.verify()?;
        let path = root.join(&parent);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let mut guard = HostOutputGuard::new(&path)?;
        for input in inputs {
            guard.protect(input)?;
        }
        guards.insert(parent, guard);
    }
    preflight(root, &guards, &planned)?;
    let guard = &guards[Path::new("")];
    let (manifest, legacy_unverified) = match existing_manifest {
        Some(existing) => existing,
        None => {
            if nonempty {
                validate_legacy(root)?;
            }
            preflight(root, &guards, &planned)?;
            guard.verify_replaceable_file(&manifest_path)?;
            let bytes = serde_json::to_vec_pretty(&Manifest {
                binding: binding.clone(),
                legacy_unverified: nonempty,
            })?;
            ensure!(
                bytes.len() as u64 <= MAX_MANIFEST,
                "binding manifest too large"
            );
            let mut staged = tempfile::NamedTempFile::new_in(root)?;
            staged.write_all(&bytes)?;
            staged.as_file().sync_all()?;
            check_file(&lock, &root.join(LOCK))?;
            guard.verify_replaceable_file(&manifest_path)?;
            // 首次内容提交前独占建立来源绑定；此文件不是导出完成标记。
            staged
                .persist_noclobber(&manifest_path)
                .map_err(|e| e.error)?;
            (read_file(guard, &manifest_path)?, nonempty)
        }
    };
    Ok(OutputTree {
        root: root.into(),
        guards,
        targets: planned,
        lock,
        manifest,
        legacy_unverified,
    })
}

fn preflight(
    root: &Path,
    guards: &BTreeMap<PathBuf, HostOutputGuard>,
    targets: &BTreeSet<PathBuf>,
) -> Result<()> {
    #[cfg(test)]
    tests::record_preflight(targets.len());
    let mut directories = Vec::new();
    for guard in guards.values() {
        guard.verify()?;
        let id = same_file::Handle::from_path(guard.output_root())?;
        ensure!(!directories.contains(&id), "aliased output directories");
        directories.push(id);
    }
    let mut files = HashSet::new();
    for target in targets {
        let guard = guards
            .get(target.parent().unwrap())
            .context("unplanned output directory")?;
        let path = root.join(target);
        guard.verify_replaceable_file(&path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let id = same_file::Handle::from_path(&path)?;
                ensure!(files.insert(id), "aliased output files");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

impl OutputTree {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn legacy_unverified(&self) -> bool {
        self.legacy_unverified
    }

    pub(crate) fn guard(&self, parent: &Path) -> Result<&HostOutputGuard> {
        relative(parent, true)?;
        self.guards.get(parent).context("unplanned output parent")
    }

    pub(crate) fn verify_all(&self) -> Result<()> {
        self.verify_tree_identity()?;
        preflight(&self.root, &self.guards, &self.targets)
    }

    fn verify_tree_identity(&self) -> Result<()> {
        #[cfg(test)]
        tests::record_tree_identity();
        check_file(&self.lock, &self.root.join(LOCK))?;
        check_file(&self.manifest, &self.root.join(MANIFEST))?;
        for guard in self.guards.values() {
            guard.verify()?;
        }
        Ok(())
    }

    /// 调用者按媒体、JSON、HTML、汇总的顺序传入；失败不回滚此前已提交文件。
    /// 并发改动下仅保证逐文件检查与替换，可能部分提交，不提供跨文件 CAS。
    pub(crate) fn publish_all(&self, entries: &[(PathBuf, PathBuf)]) -> Result<()> {
        self.publish_with(entries, |_| Ok(()))
    }

    fn publish_with(
        &self,
        entries: &[(PathBuf, PathBuf)],
        before_commit: impl Fn(usize) -> Result<()>,
    ) -> Result<()> {
        let mut committed = 0;
        let result = (|| -> Result<()> {
            self.verify_all()?;
            let mut seen = BTreeSet::new();
            let mut sources = Vec::new();
            let mut source_guards = BTreeMap::new();
            let canonical_root = fs::canonicalize(&self.root)?;
            for (dest, source) in entries {
                ensure!(self.targets.contains(dest), "unplanned output target");
                ensure!(seen.insert(dest), "duplicate publication target");
                ensure!(source.is_absolute(), "absolute staging source required");
                let parent = source.parent().context("missing staging parent")?;
                // 同一暂存父目录只固定一次祖先句柄，源文件句柄仍逐个持有。
                let source_guard = match source_guards.entry(parent.to_path_buf()) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        let guard = HostOutputGuard::new(parent)?;
                        #[cfg(test)]
                        tests::record_source_guard();
                        entry.insert(guard)
                    }
                };
                let file = read_file(source_guard, source)?;
                // 禁止把输出树作为暂存输入，已有目标的路径别名也不例外。
                let canonical = fs::canonicalize(source)?;
                ensure!(
                    !canonical.starts_with(&canonical_root),
                    "staging source is inside output tree"
                );
                let modified = file.metadata()?.modified()?;
                sources.push((file, modified));
            }
            self.verify_all()?;
            for (index, ((dest, source), (file, modified))) in
                entries.iter().zip(&sources).enumerate()
            {
                let source_guard = &source_guards[source.parent().unwrap()];
                source_guard.verify()?;
                check_file(file, source)?;
                let target = self.root.join(dest);
                let mut staged = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
                std::io::copy(&mut &*file, staged.as_file_mut())?;
                // 二次复制后恢复源修改时间，保留相册缓存视频的时间契约。
                staged
                    .as_file()
                    .set_times(fs::FileTimes::new().set_modified(*modified))?;
                staged.as_file().sync_all()?;
                before_commit(index)?;
                // 全量候选已预检两次；提交阶段只核验树身份、本次源和当前目标。
                self.verify_tree_identity()?;
                source_guard.verify()?;
                check_file(file, source)?;
                self.guard(dest.parent().unwrap())?
                    .verify_replaceable_file(&target)?;
                staged
                    .persist(&target)
                    .map_err(|e| e.error)
                    .context("SNS file replacement failed")?;
                committed += 1;
            }
            Ok(())
        })();
        result.with_context(|| format!("SNS publication stopped after {committed} committed file(s); earlier files were not rolled back"))
    }
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
