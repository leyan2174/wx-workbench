//! SNS 预览；路径显式传入，不推断账号，仅在显式授权后下载媒体。
use crate::application::moments::cache::{build_cache_index, CacheKeys, CacheLimits, CacheRoots};
use anyhow::{ensure, Context, Result};

pub use crate::service::operation_requests::export_sns::LocalCacheArgs;

impl LocalCacheArgs {
    fn keys(&self) -> Result<CacheKeys> {
        use std::io::Read;
        let mut keys = CacheKeys::default();
        if let Some(path) = &self.image_key_file {
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            std::fs::File::open(path)
                .context("无法打开图片密钥文件")?
                .take(129)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 128, "图片密钥文件过长");
            let raw = std::str::from_utf8(&bytes)
                .context("图片密钥文件须为 UTF-8")?
                .trim_start_matches('\u{feff}')
                .trim();
            ensure!(
                raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
                "图片密钥须为 32 位十六进制"
            );
            let mut key = zeroize::Zeroizing::new([0u8; 16]);
            for (index, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)?;
            }
            keys.image_aes_key = Some(*key);
        }
        if let Some(raw) = &self.image_xor_key {
            let raw = raw.trim();
            keys.image_xor_key = match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                Some(hex) => u8::from_str_radix(hex, 16),
                None => raw.parse(),
            }
            .context("图片 XOR 字节须为 0 至 255")?;
        }
        Ok(keys)
    }
}

pub use crate::service::operation_requests::export_sns::Args;

pub fn cmd_export(args: Args) -> Result<()> {
    let Args {
        sns_db,
        contact_db,
        output_dir: output,
        contacts,
        utc_offset,
        local_cache,
        download_media,
        update,
        adopt_existing,
    } = args;
    ensure!(!adopt_existing || update, "--adopt-existing 需要 --update");
    let sns_db = sns_db.canonicalize().context("找不到 SNS 数据库")?;
    let output = std::path::absolute(output)?;
    crate::infrastructure::publication::separate(
        sns_db.parent().context("SNS 数据库缺少父目录")?,
        &output,
    )?;
    let contact_db = contact_db.map(|p| p.canonicalize()).transpose()?;
    if let Some(path) = &contact_db {
        crate::infrastructure::publication::separate(
            path.parent().context("联系人数据库缺少父目录")?,
            &output,
        )?;
    }
    let raw =
        contacts.unwrap_or_else(|| std::env::var("WECHAT_EXPORT_CONTACTS").unwrap_or_default());
    let options = crate::application::moments::ExportOptions {
        timezone: utc_offset
            .map(|s| {
                s.parse::<chrono::FixedOffset>()
                    .map(crate::application::moments::TimeZone::Fixed)
            })
            .transpose()
            .context("时区偏移格式应为 +08:00 或 -05:00")?
            .unwrap_or_default(),
        contacts: raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        export_time: None,
    };
    let enabled = local_cache.xwechat_cache.is_some() || local_cache.sns_cache.is_some();
    ensure!(
        enabled || (local_cache.image_key_file.is_none() && local_cache.image_xor_key.is_none()),
        "图片密钥参数需要显式缓存目录"
    );
    let keys = local_cache.keys()?;
    let mut inputs = vec![sns_db.clone()];
    inputs.extend(contact_db.iter().cloned());
    if let Some(path) = &local_cache.image_key_file {
        inputs.push(path.canonicalize()?);
    }
    for root in [&local_cache.xwechat_cache, &local_cache.sns_cache]
        .into_iter()
        .flatten()
    {
        crate::infrastructure::publication::separate(root, &output)?;
        inputs.push(root.canonicalize()?);
    }
    let cache = if enabled {
        Some(build_cache_index(
            &CacheRoots {
                xwechat: local_cache.xwechat_cache,
                file_storage_sns: local_cache.sns_cache,
            },
            &keys,
            CacheLimits::default(),
        )?)
    } else {
        None
    };
    let recovery = cache
        .as_ref()
        .map(|index| crate::application::moments::CacheRecovery { index, keys: &keys });
    let download = download_media.then(crate::application::moments::DownloadOptions::default);
    let report = if update {
        use crate::application::moments::TimelinePublication;
        use crate::infrastructure::output_tree::ExistingPolicy;
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(b"wx-sns-static-source-v1\0");
        for path in std::iter::once(&sns_db).chain(contact_db.iter()) {
            digest.update(path.to_string_lossy().to_lowercase().as_bytes());
            digest.update([0]);
        }
        crate::application::moments::export_database_with_publication(
            &sns_db,
            contact_db.as_deref(),
            &output,
            &options,
            recovery.as_ref(),
            download.as_ref(),
            &TimelinePublication {
                flat_cache: false,
                source_kind: "snapshot".into(),
                source_id: format!("{:x}", digest.finalize()),
                policy: if adopt_existing {
                    ExistingPolicy::Adopt
                } else {
                    ExistingPolicy::Update
                },
                inputs,
            },
        )?
    } else {
        crate::application::moments::export_database_with_media(
            &sns_db,
            contact_db.as_deref(),
            &output,
            &options,
            recovery.as_ref(),
            download.as_ref(),
        )?
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "engine":"rust", "contacts":report.contacts, "posts":report.posts,
            "files":report.files, "rows_seen":report.rows_seen, "filtered":report.filtered,
            "invalid":report.invalid, "warnings":report.warnings,
            "cache_scanned":cache.as_ref().map(|index| index.scanned),
            "cache_images":cache.as_ref().map(|index| index.images().len()),
            "cache_videos":cache.as_ref().map(|index| index.videos().len()),
            "media_recovered":report.media_recovered,"media_missing":report.media_missing,"media_failed":report.media_failed,
            "media_downloaded":report.media_downloaded,"media_download_failed":report.media_download_failed,
            "legacy_unverified":report.legacy_unverified,
        }))?
    );
    ensure!(
        report.media_failed == 0,
        "{} 个媒体恢复失败；其他记录已导出，详见 _media_recovery.json",
        report.media_failed
    );
    ensure!(
        report.media_download_failed == 0,
        "{} 个媒体下载失败；其他记录已导出，详见 _media_recovery.json",
        report.media_download_failed
    );
    ensure!(
        report.invalid == 0,
        "{} 条 SNS 记录无法解析；其他记录已导出，详见 warnings",
        report.invalid
    );
    Ok(())
}
