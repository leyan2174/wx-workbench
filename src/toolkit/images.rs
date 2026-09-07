//! 图片批处理的目录布局、重复跳过及结果发布；解码算法复用 attachment 模块。

use super::{files::*, raw_config, Failure, Report};
use crate::attachment::decoder::{dispatch, V2KeyMaterial, V2_MAGIC};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn parse_aes(value: &str) -> Result<[u8; 16]> {
    ensure!(
        value.is_ascii() && value.len() >= 16,
        "Image AES key requires at least 16 ASCII bytes"
    );
    Ok(value.as_bytes()[..16].try_into().unwrap())
}

fn formatted_path(base: &Path, extension: &str) -> PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(format!(".{extension}"));
    path.into()
}
pub(super) fn parse_xor(value: &str) -> Result<u8> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Ok(u8::from_str_radix(hex, 16).context("Invalid XOR key")?)
    } else {
        Ok(value.parse().context("Invalid XOR key")?)
    }
}
fn image_keys(
    cfg: &Value,
    aes: Option<String>,
    xor: Option<String>,
) -> Result<(Option<[u8; 16]>, u8)> {
    let aes = aes.or_else(|| {
        cfg.get("image_aes_key")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    });
    let xor = xor.or_else(|| {
        cfg.get("image_xor_key").map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        })
    });
    Ok((
        aes.as_deref().map(parse_aes).transpose()?,
        xor.as_deref().map(parse_xor).transpose()?.unwrap_or(0x88),
    ))
}

pub fn decode_image(input: String, output: Option<String>) -> Result<()> {
    let (_, cfg) = raw_config()?;
    let (aes, xor) = image_keys(&cfg, None, None)?;
    let input = PathBuf::from(input);
    let decoded = dispatch(
        &fs::read(&input)?,
        V2KeyMaterial {
            aes_key: aes.as_ref(),
            xor_key: xor,
        },
    )?;
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        let stem = stem
            .strip_suffix("_t")
            .or_else(|| stem.strip_suffix("_h"))
            .unwrap_or(&stem);
        formatted_path(&input.with_file_name(stem), decoded.format)
    });
    ensure!(
        resolved(&input)? != resolved(&output)?,
        "Output would overwrite source"
    );
    atomic_output(&output, |tmp| Ok(fs::write(tmp, &decoded.data)?))?;
    println!(
        "{}",
        serde_json::json!({"output":output,"format":decoded.format,"bytes":decoded.data.len(),"engine":"rust"})
    );
    Ok(())
}

pub fn decode_images(
    input: Option<String>,
    output: Option<String>,
    aes: Option<String>,
    xor: Option<String>,
    force: bool,
) -> Result<()> {
    let (base, cfg) = raw_config()?;
    let input = match input {
        Some(p) => PathBuf::from(p),
        None => {
            let db = crate::config::load_config()?.db_dir;
            db.parent()
                .context("Database has no account parent")?
                .join("msg/attach")
        }
    };
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        base.join(
            cfg.get("decoded_image_dir")
                .and_then(Value::as_str)
                .unwrap_or("decoded_images"),
        )
    });
    let (aes, xor) = image_keys(&cfg, aes, xor)?;
    batch(&input, &output, aes.as_ref(), xor, force, true)?.finish()
}

pub fn batch_images(input: String, output: Option<String>) -> Result<()> {
    let (_, cfg) = raw_config()?;
    let (aes, xor) = image_keys(&cfg, None, None)?;
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(format!("{}_decoded", input.trim_end_matches(['\\', '/'])))
    });
    batch(Path::new(&input), &output, aes.as_ref(), xor, false, false)?.finish()
}

fn existing_output(target: &Path, album_layout: bool) -> Result<bool> {
    let parent = target.parent().context("Missing output directory")?;
    if !parent.exists() {
        return Ok(false);
    }
    let prefix = format!(
        "{}.",
        target
            .file_name()
            .context("Missing output filename")?
            .to_string_lossy()
    )
    .to_lowercase();
    for entry in fs::read_dir(parent)? {
        let name = entry?.file_name().to_string_lossy().to_lowercase();
        if name.starts_with(&prefix) && !(album_layout && name.ends_with(".tmp")) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn batch(
    input: &Path,
    output: &Path,
    aes: Option<&[u8; 16]>,
    xor: u8,
    force: bool,
    album_layout: bool,
) -> Result<Report> {
    enum Outcome {
        Written(&'static str),
        Existing,
        NoKey,
    }
    separate(input, output)?;
    let files = collect(input, "dat", false)?;
    let mut report = Report::default();
    for file in files {
        let rel = file.strip_prefix(input)?;
        let mut target = output.join(rel);
        if album_layout {
            let parts: Vec<_> = rel.components().collect();
            if parts.len() != 4 || !parts[2].as_os_str().eq_ignore_ascii_case("Img") {
                continue;
            }
            let stem = file
                .file_stem()
                .context("Missing image filename")?
                .to_string_lossy();
            let stem = stem
                .strip_suffix("_t")
                .or_else(|| stem.strip_suffix("_h"))
                .unwrap_or(&stem);
            target = output.join(parts[0]).join(parts[1]).join(stem);
        } else {
            let stem = file
                .file_stem()
                .context("Missing image filename")?
                .to_string_lossy();
            let stem = stem
                .strip_suffix("_t")
                .or_else(|| stem.strip_suffix("_h"))
                .unwrap_or(&stem);
            target.set_file_name(stem);
        }
        report.total += 1;
        let result = (|| -> Result<Outcome> {
            separate(input, &target)?;
            if !force && existing_output(&target, album_layout)? {
                return Ok(Outcome::Existing);
            }
            let bytes = fs::read(&file)?;
            // 与旧批处理一致：只跳过缺 AES 的 V2，V1 固定密钥和旧 XOR 仍继续解码。
            if aes.is_none() && bytes.starts_with(&V2_MAGIC) {
                return Ok(Outcome::NoKey);
            }
            let decoded = dispatch(
                &bytes,
                V2KeyMaterial {
                    aes_key: aes,
                    xor_key: xor,
                },
            )?;
            let target = formatted_path(&target, decoded.format);
            separate(input, &target)?;
            atomic_output(&target, |tmp| Ok(fs::write(tmp, decoded.data)?))?;
            Ok(Outcome::Written(decoded.format))
        })();
        match result {
            Ok(Outcome::Written(format)) => {
                report.written += 1;
                *report.formats.entry(format.into()).or_default() += 1;
            }
            Ok(Outcome::Existing) => report.skipped += 1,
            Ok(Outcome::NoKey) => report.skipped_no_key += 1,
            Err(e) => report.failures.push(Failure {
                path: rel.into(),
                error: e.to_string(),
            }),
        }
        if report.total % 200 == 0 {
            eprintln!(
                "Images: {} processed, {} written, {} skipped, {} skipped_no_key, {} failed",
                report.total,
                report.written,
                report.skipped,
                report.skipped_no_key,
                report.failures.len()
            );
        }
    }
    Ok(report)
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use crate::attachment::decoder::V1_MAGIC;
    use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};

    const KEY: &[u8; 16] = b"synthetic-key-16";
    const JPEG: &[u8] = b"\xff\xd8\xffsynthetic";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic";

    fn aes_image(magic: [u8; 6], key: &[u8; 16]) -> Vec<u8> {
        let padding = 16 - JPEG.len() % 16;
        let mut padded = JPEG.to_vec();
        padded.resize(JPEG.len() + padding, padding as u8);
        let aes = aes::Aes128::new(key.into());
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&(JPEG.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0);
        for chunk in padded.chunks_exact(16) {
            let mut block = GenericArray::clone_from_slice(chunk);
            aes.encrypt_block(&mut block);
            bytes.extend_from_slice(&block);
        }
        bytes
    }

    fn mixed_input(root: &Path, album: bool) -> PathBuf {
        let input = root.join("input");
        let leaf = if album { input.join("chat/2026-09/Img") } else { input.clone() };
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("v1.dat"), aes_image(V1_MAGIC, b"cfcd208495d565ef")).unwrap();
        fs::write(leaf.join("v2.dat"), aes_image(V2_MAGIC, KEY)).unwrap();
        fs::write(leaf.join("xor.dat"), PNG.iter().map(|byte| byte ^ 0x37).collect::<Vec<_>>()).unwrap();
        input
    }

    fn assert_totals(report: &Report) {
        assert_eq!(report.total, report.written + report.skipped + report.skipped_no_key + report.failures.len());
        assert_eq!(report.formats.values().sum::<usize>(), report.written);
    }

    #[test]
    fn missing_v2_key_preserves_v1_xor_and_recovers_with_key_in_both_layouts() {
        for album in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let input = mixed_input(root.path(), album);
            let output = root.path().join("output");
            let targets = if album { output.join("chat/2026-09") } else { output.clone() };
            let report = batch(&input, &output, None, 0x88, false, album).unwrap();
            assert_totals(&report);
            assert_eq!((report.total, report.written, report.skipped, report.skipped_no_key), (3, 2, 0, 1));
            assert!(report.failures.is_empty());
            assert_eq!(report.formats.get("jpg"), Some(&1));
            assert_eq!(report.formats.get("png"), Some(&1));
            assert_eq!(fs::read(targets.join("v1.jpg")).unwrap(), JPEG);
            assert_eq!(fs::read(targets.join("xor.png")).unwrap(), PNG);
            assert!(!targets.join("v2.jpg").exists());
            assert!(report.finish().is_ok());
            let json = serde_json::to_value(&report).unwrap();
            for field in ["total", "written", "skipped", "planned", "failures", "skipped_no_key", "formats"] {
                assert!(json.get(field).is_some());
            }
            let resumed = batch(&input, &output, Some(KEY), 0x88, false, album).unwrap();
            assert_totals(&resumed);
            assert_eq!((resumed.written, resumed.skipped, resumed.skipped_no_key), (1, 2, 0));
            assert!(resumed.failures.is_empty());
            assert_eq!(resumed.formats.len(), 1);
            assert_eq!(resumed.formats.get("jpg"), Some(&1));
            assert_eq!(fs::read(targets.join("v2.jpg")).unwrap(), JPEG);
            let existing = batch(&input, &output, None, 0x88, false, album).unwrap();
            assert_totals(&existing);
            assert_eq!((existing.skipped, existing.skipped_no_key), (3, 0));
            assert!(existing.formats.is_empty());
        }
    }

    #[test]
    fn wrong_aes_key_is_failure_not_missing_key_and_keeps_previous_output() {
        let root = tempfile::tempdir().unwrap();
        let input = mixed_input(root.path(), false);
        let output = root.path().join("output");
        let first = batch(&input, &output, Some(KEY), 0x88, false, false).unwrap();
        assert_eq!(first.written, 3);
        let report = batch(&input, &output, Some(b"wrong-key-16byte"), 0x88, true, false).unwrap();
        assert_totals(&report);
        assert_eq!((report.written, report.skipped_no_key, report.failures.len()), (2, 0, 1));
        assert_eq!(report.failures[0].path, Path::new("v2.dat"));
        assert_eq!(fs::read(output.join("v2.jpg")).unwrap(), JPEG);
        assert!(report.finish().is_err());
    }

    #[test]
    fn corrupt_v2_ciphertext_with_key_fails_while_other_formats_continue() {
        let root = tempfile::tempdir().unwrap();
        let input = mixed_input(root.path(), false);
        let mut corrupt = aes_image(V2_MAGIC, KEY);
        corrupt.truncate(20);
        fs::write(input.join("v2.dat"), corrupt).unwrap();
        let output = root.path().join("output");
        let report = batch(&input, &output, Some(KEY), 0x88, false, false).unwrap();
        assert_totals(&report);
        assert_eq!((report.written, report.skipped_no_key, report.failures.len()), (2, 0, 1));
        assert!(!output.join("v2.jpg").exists());
        assert_eq!(report.formats.get("jpg"), Some(&1));
        assert_eq!(report.formats.get("png"), Some(&1));
        assert!(report.finish().is_err());
    }

    #[test]
    fn missing_key_only_does_not_create_output_or_count_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input");
        fs::create_dir(&input).unwrap();
        fs::write(input.join("v2.dat"), aes_image(V2_MAGIC, KEY)).unwrap();
        let output = root.path().join("output");
        let report = batch(&input, &output, None, 0x88, false, false).unwrap();
        assert_totals(&report);
        assert_eq!(report.skipped_no_key, 1);
        assert!(report.failures.is_empty());
        assert!(report.formats.is_empty());
        assert!(!output.exists());
        assert!(report.finish().is_ok());
    }
}
