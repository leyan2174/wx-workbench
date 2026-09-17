//! Historical chat-directory cache naming. Discovery does not prove association.
use std::path::{Path, PathBuf};

pub(crate) struct ImageLayout {
    pub root: PathBuf,
    nested_images: bool,
}

impl ImageLayout {
    pub fn image_directory(&self, month: &Path) -> PathBuf {
        if self.nested_images {
            month.join("Img")
        } else {
            month.to_owned()
        }
    }
}

pub(crate) fn image_layouts(
    attach: &Path,
    msgattach: Option<&Path>,
    username: &str,
) -> Vec<ImageLayout> {
    let hash = format!("{:x}", md5::compute(username.as_bytes()));
    let mut layouts = vec![ImageLayout {
        root: attach.join(&hash),
        nested_images: true,
    }];
    if let Some(root) = msgattach {
        layouts.push(ImageLayout {
            root: root.join(hash).join("Image"),
            nested_images: false,
        });
    }
    layouts
}

pub(crate) fn month(raw: &str) -> bool {
    let b = raw.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..].iter().all(u8::is_ascii_digit)
}

/// Directory export prefers HD over full; month-priority DAT lookup prefers full.
pub(crate) fn image_candidate(filename: &str) -> Option<(String, u8)> {
    let filename = filename.to_ascii_lowercase();
    let hash = filename.get(..32)?;
    if !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let suffix = &filename[32..];
    let rank = ["_h.dat", ".dat", "_w.dat", "_t.dat", "_t_w.dat"]
        .iter()
        .position(|s| *s == suffix)?;
    Some((hash.to_owned(), rank as u8))
}

pub(crate) fn video_roots(account: &Path, attach: &Path, username: &str) -> Vec<PathBuf> {
    vec![
        account.join("msg/video"),
        attach.join(format!("{:x}", md5::compute(username.as_bytes()))),
    ]
}

pub(crate) fn video_candidate(path: &Path, digest: &str) -> bool {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    stem == digest || stem == format!("{digest}_raw")
}

#[cfg(test)]
mod tests {
    use super::*;
    const HASH: &str = "900150983cd24fb0d6963f7d28e17f72";

    #[test]
    fn directory_ranking_is_not_legacy_dat_ranking_and_unknown_names_are_ignored() {
        for (suffix, rank) in [
            ("_h.dat", 0),
            (".dat", 1),
            ("_w.dat", 2),
            ("_t.dat", 3),
            ("_t_w.dat", 4),
        ] {
            assert_eq!(
                image_candidate(&format!("{HASH}{suffix}")),
                Some((HASH.into(), rank))
            );
        }
        for filename in [
            "bad.dat".to_owned(),
            format!("{HASH}_unknown.dat"),
            format!("{HASH}.jpg"),
            "z".repeat(32) + ".dat",
        ] {
            assert_eq!(image_candidate(&filename), None);
        }
    }

    #[test]
    fn layouts_keep_both_existing_cache_families_and_video_aliases() {
        let layouts = image_layouts(Path::new("attach"), Some(Path::new("old")), "synthetic");
        let hash = format!("{:x}", md5::compute(b"synthetic"));
        assert_eq!(layouts[0].root, Path::new("attach").join(&hash));
        assert_eq!(layouts[1].root, Path::new("old").join(hash).join("Image"));
        let month = Path::new("2026-09");
        assert_eq!(layouts[0].image_directory(month), month.join("Img"));
        assert_eq!(layouts[1].image_directory(month), month);
        assert!(video_candidate(Path::new(&format!("{HASH}_raw.mp4")), HASH));
        assert!(!video_candidate(
            Path::new(&format!("{HASH}_thumbnail.mp4")),
            HASH
        ));
        assert!(super::month("2026-99")); // Historical lexical check, not calendar validation.
        assert!(!super::month("2026-9"));
    }
}
