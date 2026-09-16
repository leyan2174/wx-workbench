//! 有限本地媒体编排；只扫描显式账号根，持有路径句柄，复用现有解析及解码核心。
use super::{Media, MediaInput, Options, Row};
use crate::adapters::wechat::media::{
    attachment_content, directory_layout,
    local_read::{bounded_read, Scan},
    voice as database_media,
};
use crate::business::attachment_content::{ContainerKind, Kind, NamedKind};
use crate::{
    application::attachment_references, attachment::decoder, message::export::Target,
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

pub(super) struct Inputs {
    account: PathBuf,
    decrypted: PathBuf,
    attach: PathBuf,
    msgattach: Option<PathBuf>,
    stickers: Option<PathBuf>,
    sources: Option<Vec<database_media::DecryptedSource>>,
    resources: Vec<PathBuf>,
    emoticon_catalog: Option<PathBuf>,
    aes: Option<zeroize::Zeroizing<[u8; 16]>>,
    xor: u8,
}
impl Inputs {
    pub(super) fn from_config(
        runtime: &RuntimeContext,
        config: &Value,
        media: MediaInput<'_>,
        media_enabled: bool,
    ) -> Result<Self> {
        let sources = media.sources;
        let parent = runtime.config_path.parent().context("配置缺少父目录")?;
        let configured = |key: &str| {
            config
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let path = PathBuf::from(s);
                    if path.is_absolute() {
                        path
                    } else {
                        parent.join(path)
                    }
                })
        };
        let account = runtime
            .config
            .db_dir
            .parent()
            .context("账号数据库目录缺少父目录")?
            .to_path_buf();
        if let Some(base) = configured("wechat_base_dir") {
            ensure!(
                crate::infrastructure::publication::resolved(&base)?
                    .to_string_lossy()
                    .eq_ignore_ascii_case(
                        &crate::infrastructure::publication::resolved(&account)?.to_string_lossy()
                    ),
                "wechat_base_dir 与固定账号不一致，拒绝跨账号媒体读取"
            );
        }
        let stored = zeroize::Zeroizing::new(if media_enabled {
            media
                .image_material
                .map(|(aes, xor)| (Some(aes), xor))
                .unwrap_or((None, 0x88))
        } else {
            (None, 0x88)
        });
        let aes = stored.0.map(zeroize::Zeroizing::new);
        let xor = stored.1;
        let mut emoticon_catalog = None;
        let mut resources = if sources.is_none() {
            vec![runtime
                .config
                .decrypted_dir
                .join("message/message_resource.db")]
        } else {
            Vec::new()
        };
        let sources = sources
            .map(|sources| -> Result<Vec<database_media::DecryptedSource>> {
                ensure!(sources.len() <= 2050, "静态源清单超过上限");
                let mut names = std::collections::BTreeSet::new();
                let mut selected = Vec::new();
                let roots = [
                    crate::infrastructure::publication::resolved(&runtime.directory)?,
                    crate::infrastructure::publication::resolved(&runtime.config.decrypted_dir)?,
                ];
                for source in sources {
                    let name = source.source.replace('\\', "/").to_ascii_lowercase();
                    ensure!(names.insert(name.clone()), "静态源清单重复");
                    let actual = crate::infrastructure::publication::resolved(&source.path)?
                        .to_string_lossy()
                        .to_lowercase();
                    ensure!(
                        roots.iter().any(|root| actual.starts_with(&format!(
                            "{}\\",
                            root.to_string_lossy().to_lowercase().trim_end_matches('\\')
                        ))),
                        "静态源不在固定账号缓存或解密目录中"
                    );
                    if name == "message/message_resource.db"
                        || name
                            .strip_prefix("message/message_resource_")
                            .and_then(|s| s.strip_suffix(".db"))
                            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                    {
                        resources.push(source.path.clone());
                        continue;
                    }
                    if name == "emoticon/emoticon.db" {
                        emoticon_catalog = Some(source.path.clone());
                        continue;
                    }
                    if name == "contact/contact.db" {
                        continue;
                    }
                    let media = name
                        .strip_prefix("message/media_")
                        .and_then(|s| s.strip_suffix(".db"))
                        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
                    ensure!(
                        media || super::shard_name(&name).is_ok(),
                        "不支持的静态消息来源"
                    );
                    selected.push(database_media::DecryptedSource {
                        source: name,
                        path: source.path.clone(),
                    });
                }
                Ok(selected)
            })
            .transpose()?;
        Ok(Self {
            attach: crate::attachment::resolver::attach_root_for(&account),
            account,
            decrypted: runtime.config.decrypted_dir.clone(),
            msgattach: configured("msgattach_dir"),
            stickers: configured("emoticon_output_dir")
                .or_else(|| Some(parent.join("exported_emoticons"))),
            sources,
            resources,
            emoticon_catalog,
            aes,
            xor,
        })
    }
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.account.clone(), self.decrypted.clone()];
        paths.extend(self.msgattach.iter().cloned());
        paths.extend(self.stickers.iter().cloned());
        paths.extend(self.emoticon_catalog.iter().cloned());
        paths
    }
    fn key(&self) -> decoder::V2KeyMaterial<'_> {
        decoder::V2KeyMaterial {
            aes_key: self.aes.as_deref(),
            xor_key: self.xor,
        }
    }
}

fn marker(kind: &str, status: &str, detail: impl Into<String>) -> Media {
    Media {
        kind: kind.into(),
        status: status.into(),
        path: None,
        detail: detail.into(),
        binding: None,
    }
}

fn failure_marker(kind: &str, error: &anyhow::Error) -> Media {
    use crate::business::media::{Error, Failure, Stage};
    let classified = error
        .downcast_ref::<Error>()
        .copied()
        .or_else(|| {
            error
                .downcast_ref::<database_media::DatabaseMediaError>()
                .map(database_media::DatabaseMediaError::media_error)
        })
        .unwrap_or_else(|| Error::new(Stage::Discovery, Failure::Unavailable));
    marker(kind, "unavailable", classified.to_string())
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use crate::business::media::{Error, Failure, Stage};

    #[test]
    fn report_keeps_the_stage_without_source_error_chains() {
        let error = anyhow::Error::new(Error::new(Stage::Revalidation, Failure::StaleEvidence))
            .context("SYNTHETIC_PRIVATE_PATH_AND_KEY");
        let report = failure_marker("voice", &error);
        assert_eq!(report.status, "unavailable");
        assert_eq!(report.detail, "Media Revalidation: StaleEvidence");
        let unknown = failure_marker("file", &anyhow::anyhow!("SYNTHETIC_PRIVATE_PATH_AND_KEY"));
        assert!(!unknown.detail.contains("SYNTHETIC_PRIVATE_PATH_AND_KEY"));
        assert!(unknown.path.is_none());
    }
}

/// 同一聊天的所有媒体共用预算和暂存清单；只有最终发布步骤写入目标目录。
pub(super) struct MediaOutput<'a> {
    pub stage: &'a Path,
    pub options: &'a Options,
    pub budget: &'a mut u64,
    pub files: &'a mut BTreeMap<PathBuf, PathBuf>,
}

pub(super) fn prepare(
    inputs: &Inputs,
    target: &Target,
    row: &Row,
    output: &mut MediaOutput<'_>,
    images: &ImageCatalog,
) -> Vec<Media> {
    let Some(base) = attachment_content::message_kind(row.local_type) else {
        return Vec::new();
    };
    let kind = match base {
        Kind::Image => "image",
        Kind::Voice => "voice",
        Kind::Video => "video",
        Kind::Emoticon => "sticker",
        Kind::File => "file",
        _ => return Vec::new(),
    };
    let subtype = if base == Kind::File {
        attachment_content::legacy_container_kind(row.content.as_str().unwrap_or(""))
    } else {
        None
    };
    if base == Kind::File && subtype.is_none() {
        return Vec::new();
    }
    if !output.options.media_enabled {
        return vec![marker(kind, "disabled", "媒体导出已显式禁用")];
    }
    let result = (|| -> Result<Vec<Media>> {
        if base == Kind::File {
            let input = attachment_content::MessageInput {
                username: &target.username,
                source: &row.source,
                local_id: row.local_id,
                create_time: row.create_time.context("附件缺少时间戳")?,
                body: row.content.as_str().unwrap_or(""),
            };
            let metadata = if subtype == Some(ContainerKind::Record) {
                let first = attachment_content::parse_record_item(&input, 0)?;
                let count = first.item_count.context("合并记录缺少项目数")?;
                ensure!(count <= 1000, "合并记录超过 1000 项，不输出截断结果");
                let mut items = vec![first];
                for i in 1..count {
                    items.push(attachment_content::parse_record_item(&input, i as i64)?);
                }
                items
            } else {
                vec![attachment_content::parse_file_message(&input)?]
            };
            let mut out = Vec::new();
            for meta in metadata {
                if matches!(
                    meta.kind,
                    attachment_content::Kind::Text | attachment_content::Kind::MetadataOnly
                ) {
                    continue;
                }
                let media_kind = match meta.kind {
                    attachment_content::Kind::Image => "image",
                    attachment_content::Kind::Voice => "voice",
                    attachment_content::Kind::Video => "video",
                    _ => "file",
                };
                let prepared = reference(inputs, &meta, output);
                out.push(match prepared {
                    Ok(m) => m,
                    Err(e) => failure_marker(media_kind, &e),
                });
            }
            return Ok(out);
        }
        let media = match base {
            Kind::Image => images
                .messages
                .get(&(
                    row.source.clone(),
                    row.local_id,
                    row.create_time,
                    row.local_type,
                ))
                .cloned()
                .unwrap_or_else(|| marker("image", "unavailable", "图片资源关联缺失")),
            Kind::Voice => voice(inputs, target, row, output)?,
            Kind::Video | Kind::Emoticon => named_media(inputs, target, row, output)?,
            _ => unreachable!(),
        };
        Ok(vec![media])
    })();
    result.unwrap_or_else(|error| vec![failure_marker(kind, &error)])
}

#[derive(Default)]
pub(super) struct ImageCatalog {
    pub(super) directory: BTreeMap<String, Media>,
    messages: BTreeMap<(String, i64, Option<i64>, i64), Media>,
}

pub(super) fn image_catalog(
    inputs: &Inputs,
    target: &Target,
    rows: &[Row],
    output: &mut MediaOutput<'_>,
) -> Result<ImageCatalog> {
    if !output.options.media_enabled {
        return Ok(ImageCatalog::default());
    }
    let mut scan = Scan::new();
    let mut candidates = BTreeMap::<String, Vec<(u8, PathBuf, String)>>::new();
    for layout in directory_layout::image_layouts(
        &inputs.attach,
        inputs.msgattach.as_deref(),
        &target.username,
    ) {
        let root = &layout.root;
        for (folder, is_dir) in scan.entries(root)? {
            let name = folder.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !is_dir || !directory_layout::month(name) {
                continue;
            }
            let image_dir = layout.image_directory(&folder);
            for (path, is_dir) in scan.entries(&image_dir)? {
                if is_dir {
                    continue;
                }
                let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if let Some((hash, rank)) = directory_layout::image_candidate(filename) {
                    let entries = candidates.entry(hash).or_default();
                    ensure!(entries.len() < 128, "单图候选超过上限");
                    entries.push((rank, path, name.into()));
                }
            }
        }
    }
    let mut catalog = ImageCatalog::default();
    for (hash, mut entries) in candidates {
        entries.sort();
        let result = (|| -> Result<Media> {
            let chosen = &entries[0];
            ensure!(
                entries.get(1).is_none_or(|other| other.0 != chosen.0),
                "同级图片候选不唯一，拒绝猜测月份或来源"
            );
            let bytes = bounded_read(
                &chosen.1,
                output.stage,
                output
                    .options
                    .max_media_bytes
                    .min(*output.budget)
                    .min(crate::attachment::native_image::MAX_DAT_BYTES),
            )?;
            let decoded = decoder::restore(&bytes, inputs.key())?;
            ensure!(decoded.format != "bin", "图片解码后格式未知");
            let rel = format!("image/{}/{}.{}", chosen.2, hash, decoded.format);
            save(output, PathBuf::from(&rel), &decoded.data)?;
            Ok(Media {
                kind: if matches!(decoded.format, "jpg" | "png" | "gif" | "webp") {
                    "image"
                } else {
                    "file"
                }
                .into(),
                status: "available".into(),
                path: Some(rel),
                detail: if chosen.0 >= 3 {
                    "图片（仅本地缩略图）"
                } else {
                    "图片"
                }
                .into(),
                binding: Some("chat_directory_filename_heuristic".into()),
            })
        })();
        catalog
            .directory
            .insert(hash, result.unwrap_or_else(|e| failure_marker("image", &e)));
    }
    scan.verify()?;
    let images: Vec<_> = rows
        .iter()
        .filter(|r| attachment_content::message_kind(r.local_type) == Some(Kind::Image))
        .collect();
    if images.is_empty() {
        return Ok(catalog);
    }
    let sources = if let Some(sources) = &inputs.sources {
        sources.clone()
    } else {
        database_media::source_files(&inputs.decrypted)?
            .into_iter()
            .map(|path| {
                let source = path
                    .strip_prefix(&inputs.decrypted)
                    .context("message source outside decrypted root")?
                    .to_string_lossy()
                    .replace('\\', "/");
                Ok(database_media::DecryptedSource { source, path })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let files: Vec<_> = sources
        .into_iter()
        .filter(|source| super::shard_name(&source.source).is_ok())
        .map(|source| crate::adapters::wechat::messages::SourceFile {
            logical_name: source.source,
            path: source.path,
            kind: crate::business::messages::SourceKind::Ordinary,
        })
        .collect();
    let pins = files
        .iter()
        .map(|file| crate::attachment::local_files::Pin::open(&file.path, false))
        .collect::<Result<Vec<_>>>()?;
    let snapshot =
        crate::adapters::wechat::messages::Snapshot::open(files, [target.username.clone()])?;
    for row in images {
        let result = crate::adapters::wechat::media::image_digest(
            &snapshot,
            &crate::business::messages::MessageSelector {
                username: &target.username,
                local_id: row.local_id,
                timestamp: row.create_time,
            },
            &row.source,
            row.local_type,
            &inputs.resources,
        );
        let mut media =
            match result {
                Ok(hash) => catalog.directory.get(&hash).cloned().unwrap_or_else(|| {
                    marker("image", "unavailable", "精确图片资源或本地图片缺失")
                }),
                Err(error) => failure_marker("image", &error.into()),
            };
        if media.status == "available" {
            media.binding = Some("exact_resource_filename_heuristic".into());
        }
        catalog.messages.insert(
            (
                row.source.clone(),
                row.local_id,
                row.create_time,
                row.local_type,
            ),
            media,
        );
    }
    for pin in &pins {
        pin.verify()?;
    }
    Ok(catalog)
}

fn save(output: &mut MediaOutput<'_>, relative: PathBuf, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= output.options.max_media_bytes,
        "解码产物超过单附件限制"
    );
    if let Some(existing) = output.files.get(&relative) {
        ensure!(
            super::hash_file(existing)? == super::digest(bytes),
            "同名媒体内容冲突"
        );
        return Ok(());
    }
    *output.budget = output
        .budget
        .checked_sub(bytes.len() as u64)
        .context("聊天累计媒体预算耗尽")?;
    super::stage(output.stage, output.files, relative, bytes)
}
fn store(
    output: &mut MediaOutput<'_>,
    kind: &str,
    extension: &str,
    bytes: &[u8],
    detail: String,
    binding: String,
) -> Result<Media> {
    let extension = if extension.is_empty() {
        "bin"
    } else {
        extension
    };
    ensure!(
        extension.len() <= 16 && extension.bytes().all(|b| b.is_ascii_alphanumeric()),
        "媒体扩展名无效"
    );
    let relative = format!(
        "{kind}/{}.{}",
        super::digest(bytes),
        extension.to_ascii_lowercase()
    );
    save(output, PathBuf::from(&relative), bytes)?;
    Ok(Media {
        kind: kind.into(),
        status: "available".into(),
        path: Some(relative),
        detail,
        binding: Some(binding),
    })
}
fn voice(
    inputs: &Inputs,
    target: &Target,
    row: &Row,
    output: &mut MediaOutput<'_>,
) -> Result<Media> {
    let identity = database_media::MessageIdentity {
        username: &target.username,
        source: &row.source,
        local_id: row.local_id,
    };
    let audio = if let Some(sources) = &inputs.sources {
        database_media::resolve_voice_sources(sources, identity, row.create_time)?
    } else {
        database_media::resolve_voice(&inputs.decrypted, identity)?
    };
    ensure!(
        Some(audio.evidence.create_time) == row.create_time,
        "语音关联时间与导出消息不符"
    );
    let server = row
        .server_id
        .as_i64()
        .or_else(|| row.server_id.as_str().and_then(|s| s.parse().ok()));
    ensure!(
        server == Some(audio.evidence.server_id),
        "语音关联 server_id 与导出消息不符"
    );
    ensure!(
        audio.silk.len() as u64 <= output.options.max_media_bytes.min(*output.budget),
        "语音超过预算"
    );
    let wav = crate::infrastructure::audio::prepare_wav_bytes(&audio.silk).map_err(|_| {
        crate::business::media::Error::new(
            crate::business::media::Stage::Decode,
            crate::business::media::Failure::InvalidMaterial,
        )
    })?;
    store(
        output,
        "voice",
        "wav",
        &wav,
        "语音".into(),
        "exact_message_media_join".into(),
    )
}

fn reference(
    inputs: &Inputs,
    meta: &attachment_content::AttachmentMetadata,
    output: &mut MediaOutput<'_>,
) -> Result<Media> {
    let reference =
        attachment_references::find_reference(&inputs.account, meta)?.context("本地附件缺失")?;
    ensure!(
        reference.size <= output.options.max_media_bytes.min(*output.budget),
        "附件超过预算"
    );
    let mut file = reference.file().try_clone()?;
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(output.options.max_media_bytes.min(*output.budget) + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == reference.size
            && format!("{:x}", md5::compute(&bytes)) == reference.md5,
        "附件副本内容变化"
    );
    let binding = reference
        .warning
        .map(str::to_owned)
        .unwrap_or_else(|| "message_md5".into());
    let title = meta.title.clone();
    match meta.kind {
        attachment_content::Kind::Image => {
            let plain = decoder::detect_image_format(&bytes);
            if plain != "bin" {
                let kind = if matches!(plain, "jpg" | "png" | "gif" | "webp") {
                    "image"
                } else {
                    "file"
                };
                store(output, kind, plain, &bytes, title, binding)
            } else {
                let decoded = decoder::restore(&bytes, inputs.key())?;
                let kind = if matches!(decoded.format, "jpg" | "png" | "gif" | "webp") {
                    "image"
                } else {
                    "file"
                };
                store(output, kind, decoded.format, &decoded.data, title, binding)
            }
        }
        attachment_content::Kind::Voice => {
            let wav = crate::infrastructure::audio::prepare_wav_bytes(&bytes)?;
            store(output, "voice", "wav", &wav, title, binding)
        }
        attachment_content::Kind::Video => {
            ensure!(is_mp4(&bytes), "本地视频不是受支持的 MP4");
            store(output, "video", "mp4", &bytes, title, binding)
        }
        _ => {
            let ext = Path::new(&meta.title)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("bin");
            store(output, "file", ext, &bytes, title, binding)
        }
    }
}
fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

fn named_media(
    inputs: &Inputs,
    target: &Target,
    row: &Row,
    output: &mut MediaOutput<'_>,
) -> Result<Media> {
    let kind = if attachment_content::message_kind(row.local_type) == Some(Kind::Emoticon) {
        NamedKind::Emoticon
    } else {
        NamedKind::Video
    };
    let content = attachment_content::named_media(row.content.as_str().unwrap_or(""), kind)?;
    let hash = content.digest;
    if content.kind == NamedKind::Emoticon {
        let root = inputs
            .stickers
            .as_deref()
            .context("未配置 emoticon_output_dir；不自动下载表情")?;
        let resolved = crate::adapters::wechat::media::local_emoticon::resolve(
            root,
            inputs.emoticon_catalog.as_deref(),
            &hash,
            output.stage,
            output.options.max_media_bytes.min(*output.budget),
            output.options.max_total_media_bytes.min(500 * 1024 * 1024),
        )?;
        return store(
            output,
            "sticker",
            resolved.format,
            &resolved.bytes,
            "表情包".into(),
            resolved.binding.label().into(),
        );
    }
    let roots = directory_layout::video_roots(&inputs.account, &inputs.attach, &target.username);
    let mut scan = Scan::new();
    let mut candidates = Vec::new();
    for root in roots {
        scan.walk(
            &root,
            0,
            &|path| directory_layout::video_candidate(path, &hash),
            &mut candidates,
        )?;
    }
    ensure!(!candidates.is_empty(), "本地媒体缺失");
    let mut selected = None;
    let mut read_budget = output.options.max_total_media_bytes.min(500 * 1024 * 1024);
    for candidate in candidates {
        let bytes = bounded_read(
            &candidate,
            output.stage,
            output
                .options
                .max_media_bytes
                .min(read_budget)
                .min(*output.budget),
        )?;
        read_budget = read_budget
            .checked_sub(bytes.len() as u64)
            .context("媒体候选读取超过预算")?;
        if format!("{:x}", md5::compute(&bytes)) == hash {
            selected = Some(bytes);
            break;
        }
    }
    let bytes = selected.context("本地媒体与消息 MD5 不匹配")?;
    scan.verify()?;
    ensure!(is_mp4(&bytes), "本地视频不是受支持的 MP4");
    store(
        output,
        "video",
        "mp4",
        &bytes,
        "视频".into(),
        "message_md5".into(),
    )
}
