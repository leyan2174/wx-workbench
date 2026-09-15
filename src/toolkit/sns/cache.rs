//! Host-controlled SNS cache reads and output writes. Private formats live in the adapter.
use crate::adapters::wechat::moments::cache as adapter;
use crate::business::moments::RecoveredMediaFile;
#[cfg(test)]
use adapter::{
    apply_media_references, build_index, build_video_cache_index, decrypt_dat, detect_image_format,
    find_cached_video, image_dimensions, match_cache_images, scalar, video_cache_key, CacheMedia,
    V2,
};
pub use adapter::{
    build_cache_index, CacheIndex, CacheKeys, CacheLimits, CacheRoots, ImageEntry, RecoveryOptions,
    RecoveryReport, VideoEntry,
};
use adapter::{checked_source, is_link, reject_network_path};
use anyhow::{anyhow, bail, Context, Result};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
#[cfg(test)]
use std::time::UNIX_EPOCH;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

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
    if index.roots().iter().any(|cache| root.starts_with(cache)) {
        bail!("output must not be inside cache roots");
    }
    let path = root.join(subdir);
    if index.roots().iter().any(|cache| path.starts_with(cache)) {
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
) -> Result<RecoveredMediaFile> {
    let mut source = checked_source(&entry.path, index.roots())?;
    verify_snapshot(&source, entry.source_size(), entry.modified())?;
    let mut data = Vec::new();
    (&mut source)
        .take(index.limits().max_image_bytes.saturating_add(1))
        .read_to_end(&mut data)?;
    verify_snapshot(&source, entry.source_size(), entry.modified())?;
    let plain = adapter::decode_indexed_image(entry, &data, keys, index.limits().max_image_bytes)?;
    let format = &entry.format;
    let dir = output_directory(output, "images", index)?;
    let name = format!("{stem}_{media_index}.{format}");
    let bytes = write_new(&dir.join(&name), |target| {
        target.write_all(&plain)?;
        Ok(plain.len() as u64)
    })?;
    Ok(RecoveredMediaFile {
        relative_path: format!("images/{name}"),
        bytes,
    })
}

fn restore_video(
    index: &CacheIndex,
    entry: &VideoEntry,
    output: &Path,
    stem: &str,
    media_index: usize,
) -> Result<RecoveredMediaFile> {
    let mut source = checked_source(&entry.path, index.roots())?;
    verify_snapshot(&source, entry.source_size(), entry.modified())?;
    let mut header = [0; 16];
    let count = source.read(&mut header)?;
    adapter::validate_video_prefix(entry, &header[..count], index.limits().max_video_bytes)?;
    let dir = output_directory(output, "videos", index)?;
    let name = format!("{stem}_{media_index}.mp4");
    let bytes = write_new(&dir.join(&name), |target| {
        target.write_all(&header[..count])?;
        let remaining = index
            .limits()
            .max_video_bytes
            .saturating_sub(count as u64)
            .saturating_add(1);
        let copied = std::io::copy(&mut (&mut source).take(remaining), target)? + count as u64;
        if copied > index.limits().max_video_bytes || copied != entry.source_size() {
            bail!("cached video changed or exceeds limit");
        }
        verify_snapshot(&source, entry.source_size(), entry.modified())?;
        // 与 shutil.copy2 的主要时间语义一致，保留缓存修改时间。
        target.set_times(fs::FileTimes::new().set_modified(entry.modified()))?;
        Ok(copied)
    })?;
    Ok(RecoveredMediaFile {
        relative_path: format!("videos/{name}"),
        bytes,
    })
}

/// Compatibility host entry; parsing, selection and wire projection are adapter-owned.
pub fn recover_post_media(
    index: &CacheIndex,
    post: &Value,
    output: &Path,
    stem: &str,
    keys: &CacheKeys,
    options: RecoveryOptions,
) -> Result<RecoveryReport> {
    struct Writer<'a> {
        index: &'a CacheIndex,
        keys: &'a CacheKeys,
        output: &'a Path,
    }
    impl adapter::RecoveryWriter for Writer<'_> {
        fn image(
            &mut self,
            entry: &ImageEntry,
            stem: &str,
            media_index: usize,
        ) -> Result<RecoveredMediaFile> {
            restore_image(self.index, entry, self.keys, self.output, stem, media_index)
        }
        fn video(
            &mut self,
            entry: &VideoEntry,
            stem: &str,
            media_index: usize,
        ) -> Result<RecoveredMediaFile> {
            restore_video(self.index, entry, self.output, stem, media_index)
        }
    }
    adapter::recover_post_media(
        index,
        post,
        stem,
        options,
        &mut Writer {
            index,
            keys,
            output,
        },
    )
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod cache_tests;
