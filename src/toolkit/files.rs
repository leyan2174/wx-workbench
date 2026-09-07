//! 批处理共享的路径边界、目录遍历和同卷原子文件发布。

use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

// 先解析已存在的祖先目录及目录联接，再检查源目录和输出目录是否重叠。
pub(super) fn resolved(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.components().any(|c| c == Component::ParentDir),
        "Parent-directory components are not allowed: {}",
        path.display()
    );
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.clone();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(
            ancestor
                .file_name()
                .context("Cannot resolve path")?
                .to_os_string(),
        );
        ensure!(ancestor.pop(), "Cannot resolve path");
    }
    let mut result = ancestor.canonicalize()?;
    for name in tail.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}

pub(crate) fn separate(source: &Path, output: &Path) -> Result<()> {
    let source = resolved(source)?.to_string_lossy().to_lowercase();
    let output = resolved(output)?.to_string_lossy().to_lowercase();
    ensure!(
        source != output && !output.starts_with(&format!("{}\\", source.trim_end_matches('\\'))),
        "Output must be outside the source directory"
    );
    Ok(())
}

pub(super) fn collect(root: &Path, extension: &str, skip_migrate: bool) -> Result<Vec<PathBuf>> {
    fn walk(
        base: &Path,
        root: &Path,
        ext: &str,
        skip: bool,
        files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            // 不跟随目录联接或重解析点，避免越过账号边界或陷入目录循环。
            use std::os::windows::fs::MetadataExt;
            if fs::symlink_metadata(&path)?.file_attributes() & 0x400 != 0 {
                continue;
            }
            if entry.file_type()?.is_dir() {
                if skip && root == base && entry.file_name().eq_ignore_ascii_case("migrate") {
                    continue;
                }
                walk(base, &path, ext, skip, files)?;
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, extension, skip_migrate, &mut files)?;
    files.sort();
    Ok(files)
}

pub(crate) fn atomic_output(path: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let parent = path.parent().context("Output has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".wx-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        write(&temporary)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&temporary)?
            .sync_all()?;
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let src: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(
                PCWSTR(src.as_ptr()),
                PCWSTR(dst.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(PathBuf::from(format!("{}{suffix}", temporary.display())));
    }
    result
}
