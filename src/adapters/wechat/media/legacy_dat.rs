//! Explicit legacy DAT lookup, never a fallback from strict association.
//! Use local-time +/-31-day order (previous, current, next),
//! then sorted directory fallback; full > HD > thumbnail only within each directory.
use chrono::TimeZone;
use std::path::{Path, PathBuf};

pub(crate) trait LegacyDatIo {
    fn is_dir(&self, path: &Path) -> bool;
    fn is_file(&self, path: &Path) -> bool;
    fn directories(&self, path: &Path) -> Option<Vec<PathBuf>>;
}

pub(crate) fn find_dat_file(
    io: &impl LegacyDatIo,
    attach_root: &Path,
    chat: &str,
    file_md5: &str,
    create_time: i64,
) -> Option<PathBuf> {
    let chat_hash = format!("{:x}", md5::compute(chat.as_bytes()));
    let chat_dir = attach_root.join(&chat_hash);
    if !io.is_dir(&chat_dir) {
        return None;
    }

    // 第一步：试 create_time 当月 + 前后各一个月（共 3 个候选目录）
    let candidates_ym: Vec<String> = three_month_candidates(create_time);
    for ym in &candidates_ym {
        let img_dir = chat_dir.join(ym).join("Img");
        if let Some(p) = pick_best_in_img_dir(io, &img_dir, file_md5) {
            return Some(p);
        }
    }

    // 第二步 fallback：扫整个 chat_dir 的所有月份子目录
    let mut all_months = io.directories(&chat_dir)?;
    // 已经试过的 3 个候选可以跳过，但成本极小；保留全量扫
    all_months.sort();
    for month_dir in all_months {
        let img_dir = month_dir.join("Img");
        if let Some(p) = pick_best_in_img_dir(io, &img_dir, file_md5) {
            return Some(p);
        }
    }
    None
}

pub(crate) fn pick_best_in_img_dir(
    io: &impl LegacyDatIo,
    img_dir: &Path,
    file_md5: &str,
) -> Option<PathBuf> {
    if !io.is_dir(img_dir) {
        return None;
    }
    let full = img_dir.join(format!("{}.dat", file_md5));
    if io.is_file(&full) {
        return Some(full);
    }
    let hd = img_dir.join(format!("{}_h.dat", file_md5));
    if io.is_file(&hd) {
        return Some(hd);
    }
    let thumb = img_dir.join(format!("{}_t.dat", file_md5));
    if io.is_file(&thumb) {
        return Some(thumb);
    }
    None
}

pub(crate) fn three_month_candidates(unix_ts: i64) -> Vec<String> {
    use chrono::{Datelike, Duration};
    let dt = match chrono::Local.timestamp_opt(unix_ts, 0).single() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let prev = dt - Duration::days(31);
    let next = dt + Duration::days(31);
    [prev, dt, next]
        .iter()
        .map(|d| format!("{:04}-{:02}", d.year(), d.month()))
        .collect()
}

/// 把 `<wxchat_base>` （即 `db_storage` 父目录）拼成 `<base>/msg/attach`。
pub fn attach_root_for(wxchat_base: &Path) -> PathBuf {
    wxchat_base.join("msg").join("attach")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Default)]
    struct MemoryFiles {
        files: BTreeSet<PathBuf>,
        directories: BTreeSet<PathBuf>,
    }
    impl MemoryFiles {
        fn put(&mut self, path: PathBuf) {
            for ancestor in path.ancestors().skip(1) {
                self.directories.insert(ancestor.to_owned());
            }
            self.files.insert(path);
        }
    }
    impl LegacyDatIo for MemoryFiles {
        fn is_dir(&self, path: &Path) -> bool {
            self.directories.contains(path)
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn directories(&self, path: &Path) -> Option<Vec<PathBuf>> {
            self.is_dir(path).then(|| {
                self.directories
                    .iter()
                    .filter(|entry| entry.parent() == Some(path))
                    .cloned()
                    .rev()
                    .collect()
            })
        }
    }

    #[test]
    fn historical_previous_month_wins_before_current_even_with_lower_quality() {
        let root = Path::new("synthetic");
        let chat = root.join(format!("{:x}", md5::compute(b"alice")));
        let previous = chat.join("2025-07/Img/digest_t.dat");
        let current = chat.join("2025-08/Img/digest.dat");
        let mut io = MemoryFiles::default();
        io.put(previous.clone());
        io.put(current);
        let time = chrono::Local
            .with_ymd_and_hms(2025, 8, 15, 12, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(
            find_dat_file(&io, root, "alice", "digest", time),
            Some(previous)
        );
        assert_eq!(find_dat_file(&io, root, "bob", "digest", time), None);
    }

    #[test]
    fn invalid_timestamp_uses_sorted_legacy_fallback_not_strict_ambiguity_rules() {
        let root = Path::new("synthetic");
        let chat = root.join(format!("{:x}", md5::compute(b"alice")));
        let earlier = chat.join("2000-01/Img/digest_h.dat");
        let mut io = MemoryFiles::default();
        io.put(chat.join("2099-12/Img/digest.dat"));
        io.put(earlier.clone());
        assert!(three_month_candidates(i64::MAX).is_empty());
        assert_eq!(
            find_dat_file(&io, root, "alice", "digest", i64::MAX),
            Some(earlier)
        );
        assert_eq!(find_dat_file(&io, root, "alice", "missing", i64::MAX), None);
    }

    #[test]
    fn legacy_quality_is_full_then_hd_then_thumbnail_only_within_one_directory() {
        let dir = Path::new("synthetic/Img");
        let mut io = MemoryFiles::default();
        for suffix in ["_t.dat", "_h.dat", ".dat"] {
            let path = dir.join(format!("digest{suffix}"));
            io.put(path.clone());
            assert_eq!(pick_best_in_img_dir(&io, dir, "digest"), Some(path));
        }
    }
}
