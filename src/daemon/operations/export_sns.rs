//! SNS 预览；路径显式传入，不推断账号，仅在显式授权后下载媒体。
use crate::application::moments::cache::{CacheKeys, CacheLimits, CacheRoots};
use anyhow::{ensure, Context, Result};
use std::time::{Duration, Instant};

mod source;

pub use crate::service::operation_requests::export_sns::LocalCacheArgs;

impl LocalCacheArgs {
    fn keys(
        &self,
        material: Option<&crate::service::worker_keys::ImageMaterial>,
    ) -> Result<CacheKeys> {
        let mut keys = CacheKeys::default();
        if let Some(material) = material {
            keys.image_aes_key = Some(material.aes);
            keys.image_xor_key = material.xor;
        }
        if let Some(raw) = &self.image_xor_key {
            keys.image_xor_key = crate::application::image_publication::parse_xor(raw.trim())
                .context("图片 XOR 字节须为 0 至 255")?;
        }
        Ok(keys)
    }
}

fn guarded_cache(
    roots: &CacheRoots,
    keys: &CacheKeys,
    sources: &mut source::Sources,
) -> Result<(
    crate::application::moments::cache::CacheIndex,
    Vec<crate::attachment::image_key::VerifiedImageSamples>,
)> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bytes_left = 16 * 1024 * 1024u64;
    let mut samples = Vec::new();
    let mut before_read = |root: &std::path::Path, path: &std::path::Path, image| -> Result<()> {
        ensure!(Instant::now() < deadline, "离线 SNS 来源验证超时");
        if image {
            bytes_left = bytes_left
                .checked_sub(6)
                .context("离线 SNS 样本读取预算超限")?;
        }
        if sources.candidate(path, image)? {
            let aes = keys
                .image_aes_key
                .as_ref()
                .context("离线 SNS V2 图片缺少受保护材料；未尝试明文回退")?;
            bytes_left = bytes_left
                .checked_sub(31)
                .context("离线 SNS 样本读取预算超限")?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("离线 SNS 图片样本验证超时")?;
            samples.push(
                crate::attachment::image_key::validate_material_for_image_source(
                    root, path, aes, remaining, 31,
                )
                .map_err(|_| anyhow::anyhow!("离线 SNS 图片样本不足、已变化或材料未通过验证"))?,
            );
        }
        Ok(())
    };
    let index = crate::adapters::wechat::moments::cache::build_cache_index_checked(
        roots,
        keys,
        CacheLimits::default(),
        &mut before_read,
    )?;
    Ok((index, samples))
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
    let runtime = crate::runtime::RuntimeContext::for_operation()?;
    let config_pin = crate::service::config_pin::ConfigPin::new(&runtime)?;
    let mut sources = source::Sources::default();
    let sns_db = sources
        .database(&sns_db)
        .context("SNS 数据库来源不可固定")?;
    let output = std::path::absolute(output)?;
    crate::infrastructure::publication::separate(
        sns_db.parent().context("SNS 数据库缺少父目录")?,
        &output,
    )?;
    let contact_db = contact_db
        .as_deref()
        .map(|path| sources.database(path))
        .transpose()?;
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
        enabled || local_cache.image_xor_key.is_none(),
        "图片密钥参数需要显式缓存目录"
    );
    local_cache.validate_request()?;
    let roots = CacheRoots {
        xwechat: local_cache
            .xwechat_cache
            .as_deref()
            .map(|root| sources.directory(root))
            .transpose()?,
        file_storage_sns: local_cache
            .sns_cache
            .as_deref()
            .map(|root| sources.directory(root))
            .transpose()?,
    };
    let mut inputs = vec![sns_db.clone()];
    inputs.extend(contact_db.iter().cloned());
    for root in [&roots.xwechat, &roots.file_storage_sns]
        .into_iter()
        .flatten()
    {
        crate::infrastructure::publication::separate(root, &output)?;
        inputs.push(root.canonicalize()?);
    }
    config_pin.verify(&runtime)?;
    sources.verify()?;
    let snapshot = if enabled {
        let material = crate::service::worker_keys::image_material(&runtime)
            .map_err(|_| anyhow::anyhow!("离线 SNS 受保护图片材料不可用"))?;
        Some(crate::service::worker_keys::ImageSnapshot {
            revision: crate::service::worker_keys::expected_revision(&runtime)?,
            material,
        })
    } else {
        None
    };
    let keys = local_cache.keys(
        snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.material.as_ref()),
    )?;
    let (cache, verified_samples) = if enabled {
        crate::service::worker_keys::verify_image_revision(&runtime)?;
        let (index, samples) = guarded_cache(&roots, &keys, &mut sources)?;
        (Some(index), samples)
    } else {
        (None, Vec::new())
    };
    let verify_sources = || -> Result<()> {
        config_pin.verify(&runtime)?;
        sources.verify()?;
        Ok(())
    };
    let verify_cache = || -> Result<()> {
        for samples in &verified_samples {
            samples
                .verify()
                .map_err(|_| anyhow::anyhow!("离线 SNS 图片样本来源已变化"))?;
        }
        if enabled {
            crate::service::worker_keys::verify_image_revision(&runtime)
                .map_err(|_| anyhow::anyhow!("离线 SNS 图片材料绑定或版本已变化"))?;
        }
        Ok(())
    };
    verify_sources()?;
    verify_cache()?;
    let recovery = cache
        .as_ref()
        .map(|index| crate::application::moments::CacheRecovery {
            index,
            keys: &keys,
            verify: &verify_cache,
        });
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
        // This path hash labels the host-selected source; it does not prove account ownership.
        crate::application::moments::export_database_verified(
            &sns_db,
            contact_db.as_deref(),
            &output,
            &options,
            recovery.as_ref(),
            download.as_ref(),
            crate::application::moments::VerifiedPublication {
                verify: &verify_sources,
                policy: Some(&TimelinePublication {
                    flat_cache: false,
                    source_kind: "snapshot".into(),
                    source_id: format!("{:x}", digest.finalize()),
                    policy: if adopt_existing {
                        ExistingPolicy::Adopt
                    } else {
                        ExistingPolicy::Update
                    },
                    inputs,
                }),
            },
        )?
    } else {
        crate::application::moments::export_database_verified(
            &sns_db,
            contact_db.as_deref(),
            &output,
            &options,
            recovery.as_ref(),
            download.as_ref(),
            crate::application::moments::VerifiedPublication {
                policy: None,
                verify: &verify_sources,
            },
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
            "image_material_status": if !enabled { "not_requested" } else if verified_samples.is_empty() { "unverified" } else { "verified_local_samples" },
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

#[cfg(test)]
mod tests;
