//! WeChat cache layout and pinned, attribute-only legacy size scanning.
use crate::business::chat_plan::{Partial, ScanContribution};
use anyhow::{bail, ensure, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(crate) fn media_root(source_dir: Option<&Path>, media_dir: Option<&Path>) -> Option<PathBuf> {
    media_dir
        .map(Path::to_path_buf)
        .or_else(|| source_dir.map(|path| path.join("msg")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_layout_and_missing_lane_meaning_remain_wechat_specific() {
        let root = tempfile::tempdir().unwrap();
        let media = media_root(Some(root.path()), None).unwrap();
        assert_eq!(media, root.path().join("msg"));
        assert_eq!(
            media_root(None, Some(root.path())),
            Some(root.path().to_path_buf())
        );
        assert!(media_root(None, None).is_none());
        let hash = crate::adapters::wechat::messages::read::layout::username_hash("synthetic");
        fs::create_dir_all(media.join("attach").join(&hash)).unwrap();
        fs::write(media.join("attach").join(&hash).join("one.bin"), [1; 7]).unwrap();
        let first = scan_username(&media, "synthetic");
        assert_eq!(first.bytes, 7);
        assert!(first.statuses.is_empty());
        // An absent lane is skipped; an existing non-attach lane without the
        // username directory is limited. It must not discard prior bytes.
        fs::create_dir(media.join("file")).unwrap();
        let second = scan_username(&media, "synthetic");
        assert_eq!(second.bytes, 7);
        assert_eq!(
            second.statuses,
            std::collections::BTreeSet::from([Partial::ScanLimited])
        );
    }
}

pub(crate) type ScanTotal = ScanContribution;

pub(crate) struct ScanPin {
    _handle: fs::File,
    metadata: fs::Metadata,
}

pub(crate) fn scan_pin(path: &Path) -> std::result::Result<ScanPin, Partial> {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // 只读取属性、不跟随重解析点；拒绝共享删除，固定整个扫描链。
        options
            .access_mode(0x80)
            .share_mode(0x3)
            .custom_flags(0x02200000);
    }
    #[cfg(not(windows))]
    {
        options.read(true);
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Partial::ScanReparseSkipped);
        }
    }
    let handle = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Partial::ScanMissing
        } else {
            Partial::ScanError
        }
    })?;
    let metadata = handle.metadata().map_err(|_| Partial::ScanError)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Partial::ScanReparseSkipped);
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(Partial::ScanReparseSkipped);
    }
    Ok(ScanPin {
        _handle: handle,
        metadata,
    })
}

pub(crate) fn pin_scan_root(path: &Path) -> Result<(Vec<ScanPin>, Option<Partial>)> {
    ensure!(path.is_absolute(), "扫描目录必须是显式绝对路径");
    ensure!(
        path.as_os_str().to_string_lossy().encode_utf16().count() < 32760,
        "扫描路径过长"
    );
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                ensure!(
                    name.encode_utf16().count() <= 255
                        && !name
                            .chars()
                            .any(|c| c.is_control() || "<>:\"|?*".contains(c))
                        && !name.ends_with([' ', '.']),
                    "非法扫描目录组件"
                );
            }
            Component::ParentDir | Component::CurDir => bail!("扫描路径不能包含相对组件"),
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                ensure!(
                    matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
                    "扫描仅支持本机磁盘绝对路径"
                );
            }
            _ => {}
        }
    }
    let mut current = PathBuf::new();
    let mut pins = Vec::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        match scan_pin(&current) {
            Ok(pin) if pin.metadata.is_dir() => pins.push(pin),
            Ok(_) => return Ok((pins, Some(Partial::ScanError))),
            Err(Partial::ScanMissing) => return Ok((pins, Some(Partial::ScanBaseMissing))),
            Err(status) => return Ok((pins, Some(status))),
        }
    }
    Ok((pins, None))
}

pub(crate) fn scan_username(media: &Path, username: &str) -> ScanTotal {
    let mut total = ScanTotal::default();
    let hash = crate::adapters::wechat::messages::read::layout::username_hash(username);
    for kind in ["attach", "file", "video"] {
        let parent = media.join(kind);
        let _parent_pin = match scan_pin(&parent) {
            Ok(pin) if pin.metadata.is_dir() => pin,
            Err(Partial::ScanMissing) => continue,
            Ok(_) => {
                total.statuses.insert(Partial::ScanError);
                continue;
            }
            Err(status) => {
                total.statuses.insert(status);
                continue;
            }
        };
        let root = parent.join(&hash);
        match scan_pin(&root) {
            Ok(pin) if pin.metadata.is_dir() => scan_tree(&root, pin, 0, &mut total),
            Err(Partial::ScanMissing) => {
                if kind != "attach" {
                    total.statuses.insert(Partial::ScanLimited);
                }
            }
            Ok(_) => {
                total.statuses.insert(Partial::ScanError);
                if kind != "attach" {
                    total.statuses.insert(Partial::ScanLimited);
                }
            }
            Err(status) => {
                total.statuses.insert(status);
            }
        }
    }
    total
}

pub(crate) fn scan_tree(path: &Path, _pin: ScanPin, depth: usize, total: &mut ScanTotal) {
    // 明确的深度边界，避免恶意目录耗尽线程栈；已完成部分仍可用。
    if depth >= 128 {
        total.statuses.insert(Partial::ScanDepthLimited);
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            total.statuses.insert(Partial::ScanError);
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                total.statuses.insert(Partial::ScanError);
                continue;
            }
        };
        let child = entry.path();
        match scan_pin(&child) {
            Ok(pin) if pin.metadata.is_dir() => scan_tree(&child, pin, depth + 1, total),
            Ok(pin) if pin.metadata.is_file() => {
                // 按目录项计数，不按 inode 去重；硬链接、同内容副本均与旧版一致。
                match i64::try_from(pin.metadata.len())
                    .ok()
                    .and_then(|n| total.bytes.checked_add(n))
                {
                    Some(bytes) => total.bytes = bytes,
                    None => {
                        total.statuses.insert(Partial::ScanOverflow);
                    }
                }
            }
            Ok(_) => {
                total.statuses.insert(Partial::ScanError);
            }
            Err(status) => {
                total.statuses.insert(if status == Partial::ScanMissing {
                    Partial::ScanError
                } else {
                    status
                });
            }
        }
    }
}
