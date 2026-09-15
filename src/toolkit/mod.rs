//! 原生批处理入口和统一结果；具体数据库、图片及文件操作分别由子模块承担。

pub(crate) mod asr;
pub(crate) mod attachment_refs;
pub(crate) mod audio;
pub(crate) mod chat_delta;
pub(crate) mod chat_directory;
pub(crate) mod chat_index;
pub(crate) mod chat_merge;
pub(crate) mod chat_plan;
pub(crate) mod chat_plan_selection;
mod databases;
pub(crate) mod directory_publish;
pub(crate) mod emoticons;
pub(crate) mod export_context;
mod files;
pub(crate) mod private_file;
pub(crate) mod sns;
pub(crate) mod web;
#[cfg(test)]
pub(crate) use files::atomic_output;
pub(crate) use files::{export_protected, separate, validate_export_target, ExportTarget};
pub(crate) mod cleanup;
mod images;
pub(crate) mod legacy;
pub(crate) mod monitor;
pub(crate) mod run_status;
pub(crate) mod setup;

use anyhow::Result;
pub use databases::{decrypt, Mode as DecryptMode};
pub(crate) use images::decode_images_for;
pub use images::{batch_images, decode_image, decode_images};

/// 显式图片密钥入口复用 CLI 的 AES 字节解析规则，不读取全局配置。
pub(crate) fn parse_image_aes(value: &str) -> Result<[u8; 16]> {
    images::parse_aes(value)
}

/// 显式图片密钥入口复用 CLI 的十进制或十六进制 XOR 解析规则。
pub(crate) fn parse_image_xor(value: &str) -> Result<u8> {
    images::parse_xor(value)
}
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize)]
struct Report {
    total: usize,
    written: usize,
    skipped: usize,
    skipped_no_key: usize,
    formats: std::collections::BTreeMap<String, usize>,
    planned: usize,
    failures: Vec<Failure>,
}
#[derive(Serialize)]
struct Failure {
    path: PathBuf,
    error: String,
}
impl Report {
    fn finish(&self) -> Result<()> {
        println!("{}", serde_json::to_string_pretty(self)?);
        crate::ipc::outcome::BusinessOutcome::from_counts(
            self.written
                .saturating_add(self.skipped)
                .saturating_add(self.planned) as u64,
            self.failures.len() as u64,
        )
        .require_success()?;
        Ok(())
    }
}

fn raw_config() -> Result<(PathBuf, Value)> {
    let path = crate::config::find_config_file()?;
    let value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?)?
    } else {
        Value::Null
    };
    Ok((path.parent().unwrap_or(Path::new(".")).to_path_buf(), value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{databases::*, files::*, images::*};
    #[test]
    fn report_finish_distinguishes_partial_without_counting_missing_keys() {
        use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
        for (written, skipped, planned, failed, expected) in [
            (1, 0, 0, false, BusinessOutcome::Success),
            (1, 0, 0, true, BusinessOutcome::Partial),
            (0, 1, 0, true, BusinessOutcome::Partial),
            (0, 0, 1, true, BusinessOutcome::Partial),
            (0, 0, 0, true, BusinessOutcome::Failure),
            (0, 0, 0, false, BusinessOutcome::Success),
        ] {
            let mut report = Report {
                written,
                skipped,
                planned,
                skipped_no_key: 3,
                ..Default::default()
            };
            if failed {
                report.failures.push(Failure {
                    path: "synthetic.db".into(),
                    error: "synthetic item failure".into(),
                });
            }
            let actual = report.finish().map_or_else(
                |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
                |_| BusinessOutcome::Success,
            );
            assert_eq!(actual, expected);
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "wx-native-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_output_inside_source_and_parent_traversal() {
        let t = Temp::new();
        assert!(separate(&t.0, &t.0.join("out")).is_err());
        assert!(separate(&t.0, &t.0).is_err());
        assert!(resolved(&t.0.join("../other")).is_err());
        assert!(separate(&t.0.join("input"), &t.0.join("output")).is_ok());
    }
    #[test]
    fn failed_atomic_write_preserves_previous_file_and_cleans_temporary() {
        let t = Temp::new();
        let target = t.0.join("existing.png");
        fs::write(&target, b"previous").unwrap();
        let result = atomic_output(&target, |p| {
            fs::write(p, b"partial")?;
            anyhow::bail!("test failure")
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 1);
        atomic_output(&target, |p| Ok(fs::write(p, b"complete")?)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"complete");
    }
    #[test]
    fn mirrors_images_and_reports_corrupt_input_without_overwrite() {
        let t = Temp::new();
        let input = t.0.join("input");
        let output = t.0.join("out");
        fs::create_dir_all(input.join("nested")).unwrap();
        let plain = b"\x89PNG\r\n\x1a\nimage bytes";
        let cipher: Vec<_> = plain.iter().map(|b| b ^ 0x37).collect();
        fs::write(input.join("nested/photo.part.dat"), &cipher).unwrap();
        let report = batch(&input, &output, None, 0x88, true, false).unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(
            fs::read(output.join("nested/photo.part.png")).unwrap(),
            plain
        );
        fs::write(input.join("nested/photo.part.dat"), []).unwrap();
        let report = batch(&input, &output, None, 0x88, true, false).unwrap();
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            fs::read(output.join("nested/photo.part.png")).unwrap(),
            plain
        );
    }
    #[test]
    fn album_layout_collapses_thumbnail_suffix_and_is_idempotent() {
        let t = Temp::new();
        let input = t.0.join("input");
        let output = t.0.join("out");
        fs::create_dir_all(input.join("chat/2026-09/Img")).unwrap();
        let plain = b"\xff\xd8\xffjpeg";
        let cipher: Vec<_> = plain.iter().map(|b| b ^ 0x55).collect();
        fs::write(input.join("chat/2026-09/Img/abcdef_t.dat"), cipher).unwrap();
        assert_eq!(
            batch(&input, &output, None, 0x88, false, true)
                .unwrap()
                .written,
            1
        );
        assert!(output.join("chat/2026-09/abcdef.jpg").is_file());
        assert_eq!(
            batch(&input, &output, None, 0x88, false, true)
                .unwrap()
                .skipped,
            1
        );
    }
    #[test]
    fn keys_are_validated_without_echoing_values() {
        assert!(parse_aes("too-short").is_err());
        assert_eq!(
            parse_aes("0123456789abcdefghijklmnopqrstuv").unwrap(),
            *b"0123456789abcdef"
        );
        assert_eq!(parse_xor("0x88").unwrap(), 136);
        assert!(parse_xor("256").is_err());
        assert!(database_key("invalid").is_err());
        assert_eq!(database_key(&"42".repeat(32)).unwrap(), [0x42; 32]);
        for whitespace in [" ", "\t", "\n", "\r", "\u{b}", "\u{c}"] {
            assert_eq!(
                database_key(&vec!["42"; 32].join(whitespace)).unwrap(),
                [0x42; 32]
            );
        }
        for invalid in [
            "4 2".repeat(32),
            "42".repeat(31),
            "42".repeat(33),
            "４２".repeat(32),
        ] {
            assert!(database_key(&invalid).is_err());
        }
    }
}
