//! 独立朋友圈缓存图片归档，不要求时间线、联系人库或媒体 ID。
//! CacheIndex 的图片列表会丢弃失败项和缩略图，不能充当本模块的完整清单。
use crate::adapters::wechat::moments::cache::{CacheKeys, CacheLimits, CacheRoots};
use crate::attachment::{
    decoder::{dispatch, V2KeyMaterial},
    local_files::HostOutputGuard,
};
use crate::toolkit::directory_publish::{self as publish, Binding, ExistingPolicy};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    pub total: usize,
    pub success: usize,
    pub skipped_thumb: usize,
    pub skipped_exist: usize,
    pub failed: usize,
    pub bytes_written: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub source: String,
    pub path: PathBuf,
    // 不透传底层解码器错误，避免旧 XOR 错误包含推导密钥。
    pub stage: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ArchiveReport {
    pub output: PathBuf,
    #[serde(flatten)]
    pub counts: Counts,
    pub groups: BTreeMap<String, Counts>,
    pub failures: Vec<Failure>,
    pub warnings: Vec<String>,
    pub missing_roots: Vec<String>,
    pub directory_entries_seen: usize,
    pub incomplete: bool,
    pub legacy_unverified: bool,
}

pub struct ArchiveOptions {
    /// 已包含“朋友圈图片”的最终归档根目录。
    pub output: PathBuf,
    pub source_id: String,
    pub account_name: String,
    pub protected_inputs: Vec<PathBuf>,
    pub adopt_existing: bool,
    pub limits: CacheLimits,
}

#[derive(Clone, Debug)]
pub struct ArchiveCandidate {
    pub source: String,
    pub root: PathBuf,
    pub month: PathBuf,
    pub path: PathBuf,
}

/// 完整清单不读取图片内容；保留缩略图、不可解码文件和扁平旧缓存。
#[derive(Default)]
pub struct ArchiveIndex {
    pub candidates: Vec<ArchiveCandidate>,
    pub roots: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub missing_roots: Vec<String>,
    pub directory_entries_seen: usize,
    pub incomplete: bool,
}

fn local_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "归档路径必须是绝对路径");
    ensure!(
        !path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir)),
        "归档路径不能包含相对跳转"
    );
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        ensure!(
            matches!(
                prefix.kind(),
                std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
            ),
            "只允许本地磁盘路径"
        );
    }
    Ok(())
}

fn safe_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && !name.ends_with([' ', '.'])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "不安全的归档文件名"
    );
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !number.is_some_and(|n| matches!(
            n,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )),
        "不允许设备文件名"
    );
    Ok(())
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn projected(path: &Path) -> Result<PathBuf> {
    local_path(path)?;
    let mut existing = path;
    let mut tail = Vec::new();
    while !exists(existing)? {
        let name = existing.file_name().context("路径缺少已有祖先")?;
        safe_name(name.to_str().context("路径编码无效")?)?;
        tail.push(name.to_owned());
        existing = existing.parent().context("路径缺少父目录")?;
    }
    let meta = fs::symlink_metadata(existing)?;
    let guard = HostOutputGuard::new(if meta.is_dir() {
        existing
    } else {
        existing.parent().context("输入文件缺少父目录")?
    })?;
    if !meta.is_dir() {
        guard.verify_replaceable_file(existing)?;
        ensure!(tail.is_empty(), "路径祖先不是目录");
    }
    let mut result = fs::canonicalize(existing)?;
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    guard.verify()?;
    Ok(PathBuf::from(result.to_string_lossy().to_lowercase()))
}

/// 对不存在的输入路径也预检，不为配置、密钥或原始缓存创建输出目录。
pub fn validate_output(output: &Path, inputs: &[PathBuf]) -> Result<()> {
    let output = projected(output)?;
    for input in inputs {
        let input = projected(input)?;
        ensure!(
            !output.starts_with(&input) && !input.starts_with(&output),
            "归档输出与受保护输入重叠"
        );
    }
    Ok(())
}

fn scan_warning(index: &mut ArchiveIndex, source: &str) {
    index.incomplete = true;
    index
        .warnings
        .push(format!("{source}: 缓存目录无法完整枚举或包含不安全路径"));
}

fn children(
    index: &mut ArchiveIndex,
    path: &Path,
    limit: usize,
    source: &str,
) -> Vec<(PathBuf, bool)> {
    let result = (|| -> Result<Vec<(PathBuf, bool)>> {
        if !exists(path)? {
            return Ok(Vec::new());
        }
        let guard = HostOutputGuard::new(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            if index.directory_entries_seen >= limit {
                index.incomplete = true;
                index
                    .warnings
                    .push("缓存枚举达到 max_entries，未枚举部分不计入 total".into());
                break;
            }
            index.directory_entries_seen += 1;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            // 不跟随目录联接；文件级不安全项保留到逐项处理，计入 failed。
            use std::os::windows::fs::MetadataExt;
            if meta.is_dir() && meta.file_attributes() & 0x400 != 0 {
                scan_warning(index, source);
                continue;
            }
            if meta.is_dir() || meta.is_file() || meta.file_type().is_symlink() {
                entries.push((entry.path(), meta.is_dir()));
            }
        }
        guard.verify()?;
        Ok(entries)
    })();
    match result {
        Ok(entries) => entries,
        Err(_) => {
            scan_warning(index, source);
            Vec::new()
        }
    }
}

fn add_files(
    index: &mut ArchiveIndex,
    entries: Vec<(PathBuf, bool)>,
    root: &Path,
    month: &Path,
    source: &str,
) {
    index.candidates.extend(
        entries
            .into_iter()
            .filter(|(_, dir)| !dir)
            .map(|(path, _)| ArchiveCandidate {
                source: source.into(),
                root: root.into(),
                month: month.into(),
                path,
            }),
    );
}

/// 旧来源先处理，旧目录排序；xwechat 月份排序，分片和文件沿用枚举顺序。
/// 不调用 build_cache_index：它会预解码并丢失失败项，破坏“已有则不解码”的规则。
pub fn build_archive_index(roots: &CacheRoots, limits: CacheLimits) -> Result<ArchiveIndex> {
    ensure!(limits.max_entries > 0, "max_entries 必须大于零");
    let mut index = ArchiveIndex::default();
    for (source, root) in [
        ("wechat", &roots.file_storage_sns),
        ("xwechat", &roots.xwechat),
    ] {
        let Some(root) = root else { continue };
        local_path(root)?;
        if !exists(root)? {
            index.missing_roots.push(source.into());
            continue;
        }
        // 先固定显式根身份，不将配置根扩展到相邻账号。
        let guard = HostOutputGuard::new(root).context("显式缓存根不安全或不可访问")?;
        index.roots.push(root.clone());
        let mut entries = children(&mut index, root, limits.max_entries, source);
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if source == "wechat" {
            let has_month = entries.iter().any(|(path, dir)| {
                *dir && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.chars().count() == 7 && n.chars().nth(4) == Some('-'))
            });
            if has_month {
                for (month_dir, directory) in entries {
                    if !directory {
                        continue;
                    }
                    let month = PathBuf::from(month_dir.file_name().context("月份目录无名称")?);
                    let mut files = children(&mut index, &month_dir, limits.max_entries, source);
                    files.sort_by(|a, b| a.0.cmp(&b.0));
                    add_files(&mut index, files, root, &month, source);
                }
            } else {
                add_files(&mut index, entries, root, Path::new(""), source);
            }
        } else {
            for (month_dir, directory) in entries {
                if !directory {
                    continue;
                }
                let month = PathBuf::from(month_dir.file_name().context("月份目录无名称")?);
                let image_dir = month_dir.join("Sns/Img");
                for (shard, directory) in
                    children(&mut index, &image_dir, limits.max_entries, source)
                {
                    if !directory {
                        continue;
                    }
                    let files = children(&mut index, &shard, limits.max_entries, source);
                    add_files(&mut index, files, root, &month, source);
                }
            }
        }
        guard.verify()?;
    }
    Ok(index)
}

fn destination(candidate: &ArchiveCandidate) -> Result<Option<(PathBuf, String)>> {
    let name = candidate
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .context("缓存文件名编码无效")?;
    if name.ends_with("_t") {
        return Ok(None);
    }
    let stem = name.strip_suffix("_d").unwrap_or(name);
    safe_name(stem)?;
    if !candidate.month.as_os_str().is_empty() {
        ensure!(candidate.month.components().count() == 1, "月份目录无效");
        safe_name(candidate.month.to_str().context("月份目录编码无效")?)?;
    }
    Ok(Some((candidate.month.clone(), stem.into())))
}

fn existing_names(guard: &HostOutputGuard, limit: usize) -> Result<BTreeMap<String, PathBuf>> {
    let mut names = BTreeMap::new();
    for entry in fs::read_dir(guard.output_root())? {
        ensure!(names.len() < limit, "已有输出条目超过枚举限制");
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("已有输出文件名编码无效"))?;
        names.insert(name.to_lowercase(), entry.path());
    }
    guard.verify()?;
    Ok(names)
}

fn has_existing(names: &BTreeMap<String, PathBuf>, stem: &str) -> bool {
    // 使用字面前缀，不把缓存名称当 glob 表达式；保留同名任意扩展名跳过规则。
    let prefix = format!("{}.", stem.to_lowercase());
    names
        .range(prefix.clone()..)
        .next()
        .is_some_and(|(n, _)| n.starts_with(&prefix))
}

fn decoded(
    candidate: &ArchiveCandidate,
    keys: &CacheKeys,
    limit: u64,
) -> Result<crate::attachment::decoder::DecodedImage> {
    let guard = HostOutputGuard::new(candidate.path.parent().context("缓存文件缺少父目录")?)?;
    guard.verify_replaceable_file(&candidate.path)?;
    use std::os::windows::fs::OpenOptionsExt;
    let mut file = File::options()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(&candidate.path)?;
    ensure!(
        same_file::Handle::from_file(file.try_clone()?)?
            == same_file::Handle::from_path(&candidate.path)?,
        "缓存身份变化"
    );
    let snapshot = file.metadata()?;
    ensure!(
        snapshot.is_file() && snapshot.len() >= 6 && snapshot.len() <= limit,
        "缓存文件大小无效"
    );
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    (&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == snapshot.len()
            && file.metadata()?.modified()? == snapshot.modified()?,
        "缓存读取期间变化"
    );
    guard.verify_replaceable_file(&candidate.path)?;
    let image = dispatch(
        &bytes,
        V2KeyMaterial {
            aes_key: keys.image_aes_key.as_ref(),
            xor_key: keys.image_xor_key,
        },
    );
    // 错误文本在调用方转成固定阶段，不暴露密钥或明文。
    let image = image?;
    ensure!(
        !image.data.is_empty()
            && matches!(image.format, "jpg" | "png" | "gif" | "webp" | "bmp" | "tif"),
        "非可归档图片"
    );
    Ok(image)
}

/// 先枚举完整缓存，再逐项发布；报告满足 total = success + 两种 skipped + failed。
/// 没有可处理文件或没有缓存根时不创建输出目录。部分失败不回滚已成功的图片。
pub fn export(
    roots: &CacheRoots,
    keys: &CacheKeys,
    options: &ArchiveOptions,
) -> Result<ArchiveReport> {
    let mut inputs = options.protected_inputs.clone();
    inputs.extend(
        roots
            .file_storage_sns
            .iter()
            .chain(roots.xwechat.iter())
            .cloned(),
    );
    validate_output(&options.output, &inputs)?;
    let index = build_archive_index(roots, options.limits)?;
    let mut report = ArchiveReport {
        output: options.output.clone(),
        warnings: index.warnings,
        missing_roots: index.missing_roots,
        incomplete: index.incomplete,
        directory_entries_seen: index.directory_entries_seen,
        ..Default::default()
    };
    let plans: Vec<_> = index.candidates.iter().map(destination).collect();
    // 预声明全部可能扩展名，复用发布器的路径、目录联接、来源绑定和独占锁校验。
    let targets: BTreeSet<PathBuf> = plans
        .iter()
        .filter_map(|p| p.as_ref().ok()?.as_ref())
        .flat_map(|(month, stem)| {
            ["jpg", "png", "gif", "webp", "bmp", "tif"]
                .map(|ext| month.join(format!("{stem}.{ext}")))
        })
        .collect();
    let tree = if targets.is_empty() {
        None
    } else {
        let mut existing_inputs = Vec::new();
        for input in &inputs {
            if exists(input)? {
                existing_inputs.push(input.clone());
            }
        }
        Some(publish::prepare(
            &options.output,
            &Binding {
                version: 1,
                tree_kind: "cache_archive".into(),
                source_kind: "account".into(),
                source_id: options.source_id.clone(),
                user_name: options.account_name.clone(),
            },
            if options.adopt_existing {
                ExistingPolicy::Adopt
            } else {
                ExistingPolicy::Update
            },
            &targets.into_iter().collect::<Vec<_>>(),
            &existing_inputs,
            |_| Ok(()),
        )?)
    };
    report.legacy_unverified = tree.as_ref().is_some_and(|t| t.legacy_unverified());
    let mut known_names = BTreeMap::new();
    for (candidate, plan) in index.candidates.iter().zip(plans) {
        let mut count = Counts {
            total: 1,
            ..Default::default()
        };
        let mut stage = "filename";
        let outcome = (|| -> Result<()> {
            let Some((month, stem)) = plan? else {
                count.skipped_thumb = 1;
                return Ok(());
            };
            let tree = tree.as_ref().context("缺少归档输出树")?;
            let guard = tree.guard(&month)?;
            stage = "existing_output";
            let names = match known_names.entry(month.clone()) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(existing_names(guard, options.limits.max_entries)?)
                }
            };
            if has_existing(names, &stem) {
                let prefix = format!("{}.", stem.to_lowercase());
                // 已有普通文件可跳过；目录、硬链接和重解析点不能伪装成已归档图片。
                for (_, path) in names
                    .range(prefix.clone()..)
                    .take_while(|(name, _)| name.starts_with(&prefix))
                {
                    guard.verify_replaceable_file(path)?;
                }
                count.skipped_exist = 1;
                return Ok(());
            }
            stage = "read_or_decode";
            let image = decoded(candidate, keys, options.limits.max_image_bytes)?;
            stage = "publish";
            let name = format!("{stem}.{}", image.format);
            let target = tree.root().join(&month).join(&name);
            let mut staged = tempfile::NamedTempFile::new_in(guard.output_root())?;
            staged.write_all(&image.data)?;
            staged.as_file().sync_all()?;
            // 不使用 publish_all 的替换语义：同名产物必须不覆盖，失败只清理临时文件。
            guard.verify_replaceable_file(&target)?;
            validate_output(&options.output, &inputs)?;
            match staged.persist_noclobber(&target) {
                Ok(_) => {
                    count.success = 1;
                    count.bytes_written = image.data.len() as u64;
                }
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    guard.verify_replaceable_file(&target)?;
                    count.skipped_exist = 1;
                }
                Err(error) => return Err(error.error.into()),
            }
            names.insert(name.to_lowercase(), target);
            Ok(())
        })();
        if outcome.is_err() {
            count.failed = 1;
            report.failures.push(Failure {
                source: candidate.source.clone(),
                path: candidate
                    .path
                    .strip_prefix(&candidate.root)
                    .unwrap_or(&candidate.path)
                    .into(),
                stage: stage.into(),
            });
        }
        let group = format!("{}/{}", candidate.source, candidate.month.to_string_lossy());
        for target in [&mut report.counts, report.groups.entry(group).or_default()] {
            target.total += count.total;
            target.success += count.success;
            target.skipped_thumb += count.skipped_thumb;
            target.skipped_exist += count.skipped_exist;
            target.failed += count.failed;
            target.bytes_written += count.bytes_written;
        }
    }
    Ok(report)
}
