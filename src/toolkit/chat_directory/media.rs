//! 有限本地媒体编排；只扫描显式账号根，持有路径句柄，复用现有解析及解码核心。
use super::{Media, Options, Row};
use crate::{
    attachment::{
        decoder, image_metadata, local_files::HostOutputGuard, native_image::MessageIdentity,
    },
    message::export::Target,
    runtime::RuntimeContext,
    toolkit::{asr, attachment_refs as refs},
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

pub(super) struct Inputs {
    account: PathBuf,
    decrypted: PathBuf,
    attach: PathBuf,
    msgattach: Option<PathBuf>,
    stickers: Option<PathBuf>,
    sources: Option<Vec<asr::database_media::DecryptedSource>>,
    resources: Vec<PathBuf>,
    aes: Option<zeroize::Zeroizing<[u8; 16]>>,
    xor: u8,
}
impl Inputs {
    pub(super) fn from_config(
        runtime: &RuntimeContext,
        config: &Value,
        sources: Option<&[asr::database_media::DecryptedSource]>,
        media_enabled: bool,
    ) -> Result<Self> {
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
                crate::toolkit::files::resolved(&base)?
                    .to_string_lossy()
                    .eq_ignore_ascii_case(
                        &crate::toolkit::files::resolved(&account)?.to_string_lossy()
                    ),
                "wechat_base_dir 与固定账号不一致，拒绝跨账号媒体读取"
            );
        }
        let aes = config
            .get("image_aes_key")
            .filter(|_| media_enabled)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(crate::toolkit::parse_image_aes)
            .transpose()?
            .map(zeroize::Zeroizing::new);
        let xor = config
            .get("image_xor_key")
            .filter(|_| media_enabled)
            .filter(|v| !v.is_null())
            .map(|value| {
                crate::toolkit::parse_image_xor(
                    &value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                )
            })
            .transpose()?
            .unwrap_or(0x88);
        let mut resources = if sources.is_none() {
            vec![runtime
                .config
                .decrypted_dir
                .join("message/message_resource.db")]
        } else {
            Vec::new()
        };
        let sources = sources
            .map(
                |sources| -> Result<Vec<asr::database_media::DecryptedSource>> {
                    ensure!(sources.len() <= 2050, "静态源清单超过上限");
                    let mut names = std::collections::BTreeSet::new();
                    let mut selected = Vec::new();
                    let roots = [
                        crate::toolkit::files::resolved(&runtime.directory)?,
                        crate::toolkit::files::resolved(&runtime.config.decrypted_dir)?,
                    ];
                    for source in sources {
                        let name = source.source.replace('\\', "/").to_ascii_lowercase();
                        ensure!(names.insert(name.clone()), "静态源清单重复");
                        let actual = crate::toolkit::files::resolved(&source.path)?
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
                                .is_some_and(|s| {
                                    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                                })
                        {
                            resources.push(source.path.clone());
                            continue;
                        }
                        if name == "contact/contact.db" {
                            continue;
                        }
                        let media = name
                            .strip_prefix("message/media_")
                            .and_then(|s| s.strip_suffix(".db"))
                            .is_some_and(|s| {
                                !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                            });
                        ensure!(
                            media || super::shard_name(&name).is_ok(),
                            "不支持的静态消息来源"
                        );
                        selected.push(asr::database_media::DecryptedSource {
                            source: name,
                            path: source.path.clone(),
                        });
                    }
                    Ok(selected)
                },
            )
            .transpose()?;
        Ok(Self {
            attach: account.join("msg/attach"),
            account,
            decrypted: runtime.config.decrypted_dir.clone(),
            msgattach: configured("msgattach_dir"),
            stickers: configured("emoticon_output_dir")
                .or_else(|| Some(parent.join("exported_emoticons"))),
            sources,
            resources,
            aes,
            xor,
        })
    }
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.account.clone(), self.decrypted.clone()];
        paths.extend(self.msgattach.iter().cloned());
        paths.extend(self.stickers.iter().cloned());
        paths
    }
    fn key(&self) -> decoder::V2KeyMaterial<'_> {
        decoder::V2KeyMaterial {
            aes_key: self.aes.as_ref().map(|key| &**key),
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

pub(super) fn prepare(
    inputs: &Inputs,
    target: &Target,
    row: &Row,
    stage: &Path,
    options: &Options,
    budget: &mut u64,
    files: &mut BTreeMap<PathBuf, PathBuf>,
    images: &ImageCatalog,
) -> Vec<Media> {
    let base = row.local_type & 0xffff_ffff;
    let kind = match base {
        3 => "image",
        34 => "voice",
        43 => "video",
        47 => "sticker",
        49 => "file",
        _ => return Vec::new(),
    };
    let subtype = if base == 49 { app_type(row) } else { None };
    if base == 49 && !matches!(subtype, Some(6 | 19)) {
        return Vec::new();
    }
    if !options.media_enabled {
        return vec![marker(kind, "disabled", "媒体导出已显式禁用")];
    }
    let result = (|| -> Result<Vec<Media>> {
        if base == 49 {
            let input = refs::MessageInput {
                username: &target.username,
                source: &row.source,
                local_id: row.local_id,
                create_time: row.create_time.context("附件缺少时间戳")?,
                body: row.content.as_str().unwrap_or(""),
            };
            let metadata = if subtype == Some(19) {
                let first = refs::parse_record_item(&input, 0)?;
                let count = first.item_count.context("合并记录缺少项目数")?;
                ensure!(count <= 1000, "合并记录超过 1000 项，不输出截断结果");
                let mut items = vec![first];
                for i in 1..count {
                    items.push(refs::parse_record_item(&input, i as i64)?);
                }
                items
            } else {
                vec![refs::parse_file_message(&input)?]
            };
            let mut out = Vec::new();
            for meta in metadata {
                if matches!(meta.kind, refs::Kind::Text | refs::Kind::MetadataOnly) {
                    continue;
                }
                let media_kind = match meta.kind {
                    refs::Kind::Image => "image",
                    refs::Kind::Voice => "voice",
                    refs::Kind::Video => "video",
                    _ => "file",
                };
                let prepared = reference(inputs, &meta, stage, options, budget, files);
                out.push(match prepared {
                    Ok(m) => m,
                    Err(e) => marker(
                        media_kind,
                        "unavailable",
                        format!("记录项 {:?}: {e:#}", meta.item_index),
                    ),
                });
            }
            return Ok(out);
        }
        let media = match base {
            3 => images
                .messages
                .get(&(row.source.clone(), row.local_id))
                .cloned()
                .unwrap_or_else(|| marker("image", "unavailable", "图片资源关联缺失")),
            34 => voice(inputs, target, row, stage, options, budget, files)?,
            43 | 47 => named_media(inputs, target, row, stage, options, budget, files)?,
            _ => unreachable!(),
        };
        Ok(vec![media])
    })();
    result.unwrap_or_else(|error| vec![marker(kind, "unavailable", format!("{error:#}"))])
}

fn app_type(row: &Row) -> Option<i64> {
    let body = row.content.as_str()?;
    let body = crate::message::split_group_content(body).1;
    let doc = crate::message::xml::parse(body)?;
    let app = doc.descendants().find(|n| n.has_tag_name("appmsg"))?;
    app.children()
        .find(|n| n.has_tag_name("type"))?
        .text()?
        .trim()
        .parse()
        .ok()
}

/// 明确的扫描预算，不使用 glob 跟随目录联接；守卫活到候选读取结束。
struct Scan {
    left: usize,
    guards: Vec<HostOutputGuard>,
}
impl Scan {
    fn new() -> Self {
        Self {
            left: 20_000,
            guards: Vec::new(),
        }
    }
    fn entries(&mut self, root: &Path) -> Result<Vec<(PathBuf, bool)>> {
        match fs::symlink_metadata(root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        ensure!(self.guards.len() < 1024, "媒体目录数量超过上限");
        self.guards.push(HostOutputGuard::new(root)?);
        let mut entries = Vec::new();
        for entry in fs::read_dir(root)? {
            self.left = self
                .left
                .checked_sub(1)
                .context("媒体目录扫描条目超过上限")?;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            use std::os::windows::fs::MetadataExt;
            ensure!(
                meta.file_attributes() & 0x400 == 0 && !meta.file_type().is_symlink(),
                "媒体目录含重解析路径，拒绝跟随"
            );
            ensure!(meta.is_file() || meta.is_dir(), "媒体目录含非常规文件");
            entries.push((entry.path(), meta.is_dir()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }
    fn verify(&self) -> Result<()> {
        for guard in &self.guards {
            guard.verify()?;
        }
        Ok(())
    }
    fn walk(
        &mut self,
        root: &Path,
        depth: usize,
        accept: &impl Fn(&Path) -> bool,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        ensure!(depth <= 8, "媒体目录扫描深度超过上限");
        for (path, is_dir) in self.entries(root)? {
            if is_dir {
                self.walk(&path, depth + 1, accept, out)?;
            } else if accept(&path) {
                ensure!(out.len() < 128, "媒体候选超过上限");
                out.push(path);
            }
        }
        Ok(())
    }
}
fn bounded_read(path: &Path, stage: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut guard = HostOutputGuard::new(stage)?;
    guard.pin_input(path)?;
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "媒体文件超过单文件或剩余预算"
    );
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "媒体读取超过预算");
    guard.verify()?;
    Ok(bytes)
}
fn hash32(raw: &str) -> Result<String> {
    ensure!(
        raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
        "媒体 MD5 无效或缺失"
    );
    Ok(raw.to_ascii_lowercase())
}
fn month(raw: &str) -> bool {
    let b = raw.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..].iter().all(u8::is_ascii_digit)
}

#[derive(Default)]
pub(super) struct ImageCatalog {
    pub(super) directory: BTreeMap<String, Media>,
    messages: BTreeMap<(String, i64), Media>,
}

pub(super) fn image_catalog(
    inputs: &Inputs,
    target: &Target,
    rows: &[Row],
    stage: &Path,
    options: &Options,
    budget: &mut u64,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<ImageCatalog> {
    if !options.media_enabled {
        return Ok(ImageCatalog::default());
    }
    let username_hash = format!("{:x}", md5::compute(target.username.as_bytes()));
    let mut scan = Scan::new();
    let mut candidates = BTreeMap::<String, Vec<(u8, PathBuf, String)>>::new();
    let mut layouts = vec![(inputs.attach.join(&username_hash), true)];
    if let Some(root) = &inputs.msgattach {
        layouts.push((root.join(&username_hash).join("Image"), false));
    }
    for (root, xwechat) in layouts {
        for (folder, is_dir) in scan.entries(&root)? {
            let name = folder.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !is_dir || !month(name) {
                continue;
            }
            let image_dir = if xwechat {
                folder.join("Img")
            } else {
                folder.clone()
            };
            for (path, is_dir) in scan.entries(&image_dir)? {
                if is_dir {
                    continue;
                }
                let filename = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let Some(hash) = filename.get(..32).filter(|s| hash32(s).is_ok()) else {
                    continue;
                };
                let suffix = &filename[32..];
                // 同时保留整个联系人图片目录；缩略图只在无较高等级来源时使用。
                if let Some(rank) = ["_h.dat", ".dat", "_w.dat", "_t.dat", "_t_w.dat"]
                    .iter()
                    .position(|s| *s == suffix)
                {
                    let entries = candidates.entry(hash.into()).or_default();
                    ensure!(entries.len() < 128, "单图候选超过上限");
                    entries.push((rank as u8, path, name.into()));
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
                stage,
                options
                    .max_media_bytes
                    .min(*budget)
                    .min(crate::attachment::native_image::MAX_DAT_BYTES),
            )?;
            let decoded = decoder::dispatch(&bytes, inputs.key())?;
            ensure!(decoded.format != "bin", "图片解码后格式未知");
            let rel = format!("image/{}/{}.{}", chosen.2, hash, decoded.format);
            save(
                stage,
                files,
                PathBuf::from(&rel),
                &decoded.data,
                options,
                budget,
            )?;
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
        catalog.directory.insert(
            hash,
            result.unwrap_or_else(|e| marker("image", "unavailable", format!("{e:#}"))),
        );
    }
    scan.verify()?;
    let images: Vec<_> = rows
        .iter()
        .filter(|r| r.local_type & 0xffff_ffff == 3)
        .collect();
    let mut counts = BTreeMap::new();
    for row in &images {
        *counts
            .entry((row.local_id, row.create_time, row.local_type))
            .or_insert(0usize) += 1;
    }
    // 资源库没有消息分片列；跨分片同身份不能冒充唯一关联。
    for page in images.chunks(1000) {
        let identities: Vec<_> = page
            .iter()
            .filter(|r| r.create_time.is_some())
            .map(|row| {
                (
                    MessageIdentity {
                        username: target.username.clone(),
                        source: row.source.clone(),
                        local_id: row.local_id,
                        create_time: row.create_time.unwrap(),
                        local_type: row.local_type,
                    },
                    counts[&(row.local_id, row.create_time, row.local_type)] > 1,
                )
            })
            .collect();
        let mut matches = vec![(None::<String>, None::<String>); identities.len()];
        for resource in &inputs.resources {
            match image_metadata::read_page(Some(resource), None, &identities) {
                Ok(metadata) => {
                    for (slot, meta) in matches.iter_mut().zip(metadata) {
                        if let Some(hash) = meta.md5 {
                            if slot.0.replace(hash.to_ascii_lowercase()).is_some() {
                                slot.1 = Some("多个资源分片命中同一图片身份".into());
                            }
                        } else if meta.resource_status != "missing" {
                            slot.1 = Some(format!("图片资源状态：{}", meta.resource_status));
                        }
                    }
                }
                Err(error) => {
                    for slot in &mut matches {
                        slot.1 = Some(format!("图片资源读取失败：{error:#}"));
                    }
                }
            }
        }
        for ((identity, _), (hash, error)) in identities.into_iter().zip(matches) {
            let mut media = if let Some(error) = error {
                marker("image", "unavailable", error)
            } else {
                hash.as_deref()
                    .and_then(|hash| catalog.directory.get(hash))
                    .cloned()
                    .unwrap_or_else(|| marker("image", "unavailable", "精确图片资源或本地图片缺失"))
            };
            if media.status == "available" {
                media.binding = Some("exact_resource_filename_heuristic".into());
            }
            catalog
                .messages
                .insert((identity.source, identity.local_id), media);
        }
    }
    Ok(catalog)
}

fn save(
    stage: &Path,
    files: &mut BTreeMap<PathBuf, PathBuf>,
    relative: PathBuf,
    bytes: &[u8],
    options: &Options,
    budget: &mut u64,
) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= options.max_media_bytes,
        "解码产物超过单附件限制"
    );
    if let Some(existing) = files.get(&relative) {
        ensure!(
            super::hash_file(existing)? == super::digest(bytes),
            "同名媒体内容冲突"
        );
        return Ok(());
    }
    *budget = budget
        .checked_sub(bytes.len() as u64)
        .context("聊天累计媒体预算耗尽")?;
    super::stage(stage, files, relative, bytes)
}
fn store(
    stage: &Path,
    files: &mut BTreeMap<PathBuf, PathBuf>,
    kind: &str,
    extension: &str,
    bytes: &[u8],
    detail: String,
    binding: String,
    options: &Options,
    budget: &mut u64,
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
    save(
        stage,
        files,
        PathBuf::from(&relative),
        bytes,
        options,
        budget,
    )?;
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
    stage: &Path,
    options: &Options,
    budget: &mut u64,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<Media> {
    let identity = asr::database_media::MessageIdentity {
        username: &target.username,
        source: &row.source,
        local_id: row.local_id,
    };
    let audio = if let Some(sources) = &inputs.sources {
        asr::database_media::resolve_voice_sources(sources, identity, row.create_time)?
    } else {
        asr::database_media::resolve_voice(&inputs.decrypted, identity)?
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
        audio.silk.len() as u64 <= options.max_media_bytes.min(*budget),
        "语音超过预算"
    );
    let wav = asr::prepare_wav_bytes(&audio.silk)?;
    store(
        stage,
        files,
        "voice",
        "wav",
        &wav,
        "语音".into(),
        "exact_message_media_join".into(),
        options,
        budget,
    )
}

fn reference(
    inputs: &Inputs,
    meta: &refs::AttachmentMetadata,
    stage: &Path,
    options: &Options,
    budget: &mut u64,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<Media> {
    let reference = refs::find_reference(&inputs.account, meta)?.context("本地附件缺失")?;
    ensure!(
        reference.size <= options.max_media_bytes.min(*budget),
        "附件超过预算"
    );
    let mut file = reference.file().try_clone()?;
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(options.max_media_bytes.min(*budget) + 1)
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
        refs::Kind::Image => {
            let plain = decoder::detect_image_format(&bytes);
            if plain != "bin" {
                let kind = if matches!(plain, "jpg" | "png" | "gif" | "webp") {
                    "image"
                } else {
                    "file"
                };
                store(
                    stage, files, kind, plain, &bytes, title, binding, options, budget,
                )
            } else {
                let decoded = decoder::dispatch(&bytes, inputs.key())?;
                let kind = if matches!(decoded.format, "jpg" | "png" | "gif" | "webp") {
                    "image"
                } else {
                    "file"
                };
                store(
                    stage,
                    files,
                    kind,
                    decoded.format,
                    &decoded.data,
                    title,
                    binding,
                    options,
                    budget,
                )
            }
        }
        refs::Kind::Voice => {
            let wav = asr::prepare_wav_bytes(&bytes)?;
            store(
                stage, files, "voice", "wav", &wav, title, binding, options, budget,
            )
        }
        refs::Kind::Video => {
            ensure!(is_mp4(&bytes), "本地视频不是受支持的 MP4");
            store(
                stage, files, "video", "mp4", &bytes, title, binding, options, budget,
            )
        }
        _ => {
            let ext = Path::new(&meta.title)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("bin");
            store(
                stage, files, "file", ext, &bytes, title, binding, options, budget,
            )
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
    stage: &Path,
    options: &Options,
    budget: &mut u64,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<Media> {
    let sticker = row.local_type & 0xffff_ffff == 47;
    let body = crate::message::split_group_content(row.content.as_str().unwrap_or("")).1;
    let doc = crate::message::xml::parse(body).context("媒体 XML 无效")?;
    let node = doc
        .descendants()
        .find(|n| n.has_tag_name(if sticker { "emoji" } else { "videomsg" }))
        .context("媒体 XML 缺少节点")?;
    let hash = hash32(node.attribute("md5").unwrap_or(""))?;
    let roots = if sticker {
        vec![inputs
            .stickers
            .clone()
            .context("未配置 emoticon_output_dir；不自动下载表情")?]
    } else {
        vec![
            inputs.account.join("msg/video"),
            inputs
                .attach
                .join(format!("{:x}", md5::compute(target.username.as_bytes()))),
        ]
    };
    let mut scan = Scan::new();
    let mut candidates = Vec::new();
    for root in roots {
        scan.walk(
            &root,
            0,
            &|path| {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                stem == hash || (!sticker && stem == format!("{hash}_raw"))
            },
            &mut candidates,
        )?;
    }
    ensure!(!candidates.is_empty(), "本地媒体缺失");
    let mut selected = None;
    let mut read_budget = options.max_total_media_bytes.min(500 * 1024 * 1024);
    for candidate in candidates {
        let bytes = bounded_read(
            &candidate,
            stage,
            options.max_media_bytes.min(read_budget).min(*budget),
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
    if sticker {
        let ext = decoder::detect_image_format(&bytes);
        ensure!(
            matches!(ext, "jpg" | "png" | "gif" | "webp"),
            "表情不是可展示的本地图片"
        );
        store(
            stage,
            files,
            "sticker",
            ext,
            &bytes,
            "表情包".into(),
            "message_md5".into(),
            options,
            budget,
        )
    } else {
        ensure!(is_mp4(&bytes), "本地视频不是受支持的 MP4");
        store(
            stage,
            files,
            "video",
            "mp4",
            &bytes,
            "视频".into(),
            "message_md5".into(),
            options,
            budget,
        )
    }
}
