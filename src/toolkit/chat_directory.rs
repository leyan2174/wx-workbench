//! legacy export_messages 的目录契约；媒体、渲染和发布分别处理，不自行查询消息库。
mod media;
mod render;

use super::sns::publish::{self, Binding, ExistingPolicy};
use crate::{
    attachment::local_files::HostOutputGuard, message::export::Target, runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const INVENTORY: &str = "_directory_export.json";
const MAX_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Html,
    Json,
}
impl Format {
    pub fn parse_list(raw: &str) -> Result<BTreeSet<Self>> {
        let mut formats = BTreeSet::new();
        for part in raw.split(',') {
            formats.insert(match part.trim().to_ascii_lowercase().as_str() {
                "csv" => Self::Csv,
                "html" => Self::Html,
                "json" => Self::Json,
                _ => anyhow::bail!("不支持的导出格式：{part}"),
            });
        }
        ensure!(!formats.is_empty(), "导出格式不能为空");
        Ok(formats)
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Html => "html",
            Self::Json => "json",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub formats: BTreeSet<Format>,
    pub media_enabled: bool,
    pub update: bool,
    pub max_media_bytes: u64,
    pub max_total_media_bytes: u64,
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.formats.is_empty(), "导出格式不能为空");
        ensure!(
            self.max_media_bytes > 0 && self.max_media_bytes <= 500 * 1024 * 1024,
            "单附件上限必须在 1..500MiB"
        );
        ensure!(
            self.max_total_media_bytes >= self.max_media_bytes,
            "累计媒体预算不能小于单附件上限"
        );
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub username: String,
    pub output: PathBuf,
    pub messages: usize,
    pub files: usize,
    pub media_issues: usize,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Media {
    pub kind: String,
    pub status: String,
    pub path: Option<String>,
    pub detail: String,
    pub binding: Option<String>,
}

/// 详细目录 IPC 每行保留 message/export 字段，另外补齐下列六项：
/// local_type、server_id、sort_seq、status、sender_username、raw_content。
/// raw_content 是已解压正文（NULL 可保留）；其他原始可空列必须显式出现。
/// is_received 可显式提供，否则仅个人聊天按精确 sender_username 计算。
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub local_id: i64,
    pub server_id: Value,
    #[serde(rename = "type")]
    pub local_type: i64,
    pub type_name: String,
    pub sort_seq: Value,
    pub sender_username: String,
    pub sender: String,
    pub create_time: Option<i64>,
    pub time_str: String,
    pub status: Value,
    pub content: Value,
    pub display_content: String,
    pub is_system: bool,
    pub is_received: bool,
    pub source: String,
    pub media: Vec<Media>,
    /// 保留已有核心解析出的卡片、引用等扩展信息。
    pub native: Value,
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value.get(key).with_context(|| {
        format!("ExportDirectoryByUsername 缺少目录契约字段 {key}；拒绝伪造 legacy 输出")
    })
}
impl Row {
    fn from_native(value: &Value, target: &Target) -> Result<Self> {
        let source = required(value, "source")?
            .as_str()
            .context("source 必须是字符串")?
            .replace('\\', "/");
        shard_name(&source)?;
        let local_id = required(value, "local_id")?
            .as_i64()
            .context("local_id 必须是 i64")?;
        let local_type = required(value, "local_type")?
            .as_i64()
            .context("local_type 必须是 i64")?;
        let raw = required(value, "raw_content")?;
        ensure!(
            raw.is_null() || raw.is_string(),
            "raw_content 必须是已解压字符串或 null"
        );
        let sender_value = required(value, "sender_username")?;
        ensure!(
            sender_value.is_null() || sender_value.is_string(),
            "sender_username 必须为字符串或 null"
        );
        let sender_username = sender_value.as_str().unwrap_or("").to_owned();
        let timestamp = required(value, "timestamp")?;
        ensure!(
            timestamp.is_null() || timestamp.as_i64().is_some(),
            "timestamp 类型无效"
        );
        let create_time = timestamp.as_i64();
        let time_str = create_time
            .map(|ts| {
                use chrono::TimeZone;
                chrono::Local
                    .timestamp_opt(ts, 0)
                    .single()
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .context("消息时间超出范围")
            })
            .transpose()?
            .unwrap_or_default();
        let base = local_type & 0xffff_ffff;
        let mut type_name = match base {
            1 => "文本",
            3 => "图片",
            34 => "语音",
            42 => "名片",
            43 => "视频",
            47 => "表情包",
            48 => "位置",
            49 => "分享/文件/小程序",
            10000 => "系统消息",
            10002 => "系统通知",
            _ => "未知",
        }
        .to_owned();
        if type_name == "未知" {
            type_name = format!("未知({local_type})");
        }
        let display_content = render::friendly(base, raw.as_str().unwrap_or(""), value);
        let is_received = value
            .get("is_received")
            .and_then(Value::as_bool)
            .unwrap_or(target.is_group || sender_username == target.username);
        for key in ["server_id", "sort_seq", "status"] {
            let item = required(value, key)?;
            ensure!(
                item.is_null()
                    || item.as_i64().is_some()
                    || (key == "server_id" && item.as_str().is_some()),
                "{key} 类型无效"
            );
        }
        Ok(Self {
            local_id,
            server_id: value["server_id"].clone(),
            local_type,
            type_name,
            sort_seq: value["sort_seq"].clone(),
            sender_username,
            sender: required(value, "sender")?
                .as_str()
                .context("sender 必须是字符串")?
                .into(),
            create_time,
            time_str,
            status: value["status"].clone(),
            content: raw.clone(),
            display_content,
            is_system: matches!(base, 10000 | 10002),
            is_received,
            source,
            media: Vec::new(),
            native: value.clone(),
        })
    }
}

fn shard_name(source: &str) -> Result<&str> {
    let name = source
        .strip_prefix("message/")
        .context("来源必须为 message/message_N.db")?;
    let number = name
        .strip_prefix("message_")
        .and_then(|s| s.strip_suffix(".db"))
        .context("无效分片来源")?;
    ensure!(
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        "无效分片来源"
    );
    Ok(name)
}

pub fn directory_name(target: &Target) -> String {
    let cleaned: String = target
        .chat
        .chars()
        .take(64)
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    // 固定后缀消除显示名同名、大小写别名及 Windows 设备名问题。
    format!(
        "{}--{:x}",
        if cleaned.is_empty() { "chat" } else { cleaned },
        md5::compute(target.username.as_bytes())
    )
}

pub fn read_config(runtime: &RuntimeContext) -> Result<Value> {
    // 配置可以含图片密钥，限制长度；只读显式路径，不触发账号发现。
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    fs::File::open(&runtime.config_path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "配置文件过大");
    // Value 不实现 Zeroize，返回值由调用方局部持有，不写入任何导出产物。
    let value: Value = serde_json::from_slice(&bytes)?;
    let parent = runtime.config_path.parent().context("配置缺少父目录")?;
    for (key, expected) in [
        ("db_dir", &runtime.config.db_dir),
        ("decrypted_dir", &runtime.config.decrypted_dir),
    ] {
        if let Some(raw) = value.get(key).and_then(Value::as_str) {
            let path = PathBuf::from(raw);
            let actual = if path.is_absolute() {
                path
            } else {
                parent.join(path)
            };
            ensure!(
                super::files::resolved(&actual)?
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&super::files::resolved(expected)?.to_string_lossy()),
                "固定账号后配置路径发生变化：{key}"
            );
        }
    }
    Ok(value)
}

#[derive(Serialize, Deserialize)]
struct Inventory {
    version: u32,
    runtime_id: String,
    username: String,
    files: BTreeMap<String, String>,
    messages: BTreeMap<String, String>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_file(path: &Path) -> Result<String> {
    let guard = HostOutputGuard::new(path.parent().context("文件缺少父目录")?)?;
    guard.verify_replaceable_file(path)?;
    // pin_input 要求源和输出分离，读取旧产物时使用共享只读句柄。
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    let mut hash = Sha256::new();
    let mut reader = file.take(MAX_DOCUMENT_BYTES + 1);
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= MAX_DOCUMENT_BYTES, "旧产物超过读取上限");
        hash.update(&buffer[..n]);
    }
    guard.verify()?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn export_document(
    runtime: &RuntimeContext,
    config: &Value,
    target: &Target,
    document: &Value,
    output: &Path,
    options: &Options,
) -> Result<Report> {
    export_document_impl(runtime, config, target, document, output, options, None)
}

/// sources 必须由固定账号的可信宿主提供完整静态清单；路径限制在当前账号缓存或解密树。
/// resource 和 voice 不混用旧静态目录；清单缺少所需文件时生成明确媒体缺失标记。
pub fn export_document_with_sources(
    runtime: &RuntimeContext,
    config: &Value,
    target: &Target,
    document: &Value,
    output: &Path,
    options: &Options,
    sources: &[crate::toolkit::asr::database_media::DecryptedSource],
) -> Result<Report> {
    export_document_impl(
        runtime,
        config,
        target,
        document,
        output,
        options,
        Some(sources),
    )
}

fn export_document_impl(
    runtime: &RuntimeContext,
    config: &Value,
    target: &Target,
    document: &Value,
    output: &Path,
    options: &Options,
    sources: Option<&[crate::toolkit::asr::database_media::DecryptedSource]>,
) -> Result<Report> {
    options.validate()?;
    ensure!(
        document["username"].as_str() == Some(target.username.as_str()),
        "后台聊天身份与请求不符"
    );
    ensure!(
        document
            .get("is_group")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            == target.is_group,
        "后台群聊身份不符"
    );
    let messages = document["messages"]
        .as_array()
        .context("导出缺少 messages 数组")?;
    let mut rows: Vec<Row> = messages
        .iter()
        .map(|m| Row::from_native(m, target))
        .collect::<Result<_>>()?;
    let mut identities = BTreeMap::new();
    for row in &rows {
        let key = format!("{}:{}", row.source, row.local_id);
        // 更新只接受原始消息未变的增量扩展，媒体补齐不影响消息摘要。
        let fingerprint = digest(&serde_json::to_vec(&json!([
            row.local_id,
            row.source,
            row.local_type,
            row.server_id,
            row.sort_seq,
            row.status,
            row.create_time,
            row.sender_username,
            row.content
        ]))?);
        ensure!(
            identities.insert(key, fingerprint).is_none(),
            "重复分片/local_id，不能安全导出"
        );
    }
    let mut protected = super::export_protected(runtime);
    let inputs = media::Inputs::from_config(runtime, config, sources, options.media_enabled)?;
    protected.extend(inputs.paths());
    super::validate_export_target(runtime, output)?;
    for path in &protected {
        super::separate(path, output)?;
    }
    // 缺失目录不创建；最近已有祖先由文件发布核心核验，完整路径继续作隔离检查。
    protected.retain(|p| p.exists());
    let staging = tempfile::Builder::new()
        .prefix("wx-chat-directory-")
        .tempdir()?;
    let mut staged: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut media_issues = 0;
    let mut budget = options.max_total_media_bytes;
    let mut media_output = media::MediaOutput {
        stage: staging.path(),
        options,
        budget: &mut budget,
        files: &mut staged,
    };
    let images = media::image_catalog(&inputs, target, &rows, &mut media_output)?;
    media_issues += images
        .directory
        .values()
        .filter(|m| m.status != "available")
        .count();
    for row in &mut rows {
        row.media = media::prepare(&inputs, target, row, &mut media_output, &images);
        media_issues += row
            .media
            .iter()
            .filter(|m| m.status != "available" && m.status != "disabled")
            .count();
    }
    if let Some(warnings) = document.get("metadata_warnings") {
        diagnostics.push(warnings.to_string());
    }
    if media_issues > 0 {
        diagnostics.push(format!(
            "{media_issues} 项媒体不可用，逐条原因见 media 和 _media_manifest.json"
        ));
    }
    let mut by_shard: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    if let Some(sources) = document.get("sources") {
        for source in sources.as_array().context("sources 必须是分片名数组")? {
            let name = source
                .as_str()
                .context("sources 必须含字符串")?
                .replace('\\', "/");
            by_shard.entry(shard_name(&name)?.into()).or_default();
        }
    }
    for row in &rows {
        by_shard
            .entry(shard_name(&row.source)?.into())
            .or_default()
            .push(row);
    }
    ensure!(
        !by_shard.is_empty(),
        "空消息导出需要 sources 数组保留空分片，不生成只有元数据的假成功"
    );
    for shard in by_shard.values_mut() {
        shard.sort_by_key(|r| r.sort_seq.as_i64());
    }
    for (name, shard) in &by_shard {
        for format in &options.formats {
            let bytes = render::document(*format, target, shard, staging.path(), &staged)?;
            stage(
                staging.path(),
                &mut staged,
                PathBuf::from(format!("{name}.{}", format.extension())),
                &bytes,
            )?;
        }
    }
    stage(
        staging.path(),
        &mut staged,
        PathBuf::from(".info"),
        render::info(target, document).as_bytes(),
    )?;
    stage(
        staging.path(),
        &mut staged,
        PathBuf::from("_media_manifest.json"),
        &serde_json::to_vec_pretty(&json!({
        "username":target.username,"enabled":options.media_enabled,"issues":media_issues,
        "directory_images":images.directory,
            "messages":rows.iter().filter(|r| !r.media.is_empty()).map(|r| json!({"source":r.source,"local_id":r.local_id,"media":r.media})).collect::<Vec<_>>()
        }))?,
    )?;
    let mut inventory = Inventory {
        version: 1,
        runtime_id: runtime.id.clone(),
        username: target.username.clone(),
        files: BTreeMap::new(),
        messages: identities,
    };
    // 在发布核心的目录锁下检查旧清单，未知旧文件始终不删除。
    let targets: Vec<_> = staged
        .keys()
        .cloned()
        .chain(std::iter::once(PathBuf::from(INVENTORY)))
        .collect();
    super::validate_export_target(runtime, output)?;
    let tree = publish::prepare(
        output,
        &Binding {
            version: 1,
            tree_kind: "chat_directory".into(),
            source_kind: "runtime".into(),
            source_id: runtime.id.clone(),
            user_name: target.username.clone(),
        },
        if options.update {
            ExistingPolicy::Update
        } else {
            ExistingPolicy::Reject
        },
        &targets,
        &protected,
        |_| anyhow::bail!("不自动认领旧目录"),
    )?;
    if let Some(old) = publish::read_legacy_json(&output.join(INVENTORY), MAX_DOCUMENT_BYTES)? {
        ensure!(options.update, "已有产物需要 --update");
        let old: Inventory = serde_json::from_value(old)?;
        ensure!(
            old.version == 1 && old.runtime_id == runtime.id && old.username == target.username,
            "旧产物账号绑定不符"
        );
        for (key, expected) in &old.messages {
            ensure!(
                inventory.messages.get(key) == Some(expected),
                "旧消息消失或原始内容变化，拒绝覆盖：{key}"
            );
        }
        for (name, expected) in &old.files {
            safe_relative(name)?;
            ensure!(
                hash_file(&output.join(name))? == *expected,
                "旧产物被修改，拒绝覆盖：{name}"
            );
        }
        // 不在旧清单内的同名文件属于用户，不能借 update 覆盖。
        for rel in staged.keys() {
            let name = rel.to_string_lossy().replace('\\', "/");
            ensure!(
                !output.join(rel).exists() || old.files.contains_key(&name),
                "未知旧文件不能覆盖：{name}"
            );
        }
        inventory.files = old.files;
    } else {
        for rel in staged.keys() {
            ensure!(
                !output.join(rel).exists(),
                "目录缺少完整清单，拒绝覆盖已有产物"
            );
        }
    }
    for (rel, path) in &staged {
        inventory
            .files
            .insert(rel.to_string_lossy().replace('\\', "/"), hash_file(path)?);
    }
    stage(
        staging.path(),
        &mut staged,
        PathBuf::from(INVENTORY),
        &serde_json::to_vec_pretty(&inventory)?,
    )?;
    // 媒体先发布，消息随后，清单最后；核心明确报告中途失败，不声称整树事务。
    let mut entries: Vec<_> = staged
        .iter()
        .filter(|(r, _)| r.as_path() != Path::new(INVENTORY))
        .map(|(r, p)| (r.clone(), p.clone()))
        .collect();
    entries.sort_by_key(|(r, _)| (r.components().count() == 1, r.clone()));
    entries.push((
        PathBuf::from(INVENTORY),
        staged[Path::new(INVENTORY)].clone(),
    ));
    super::validate_export_target(runtime, output)?;
    tree.publish_all(&entries)?;
    Ok(Report {
        username: target.username.clone(),
        output: output.into(),
        messages: rows.len(),
        files: staged.len(),
        media_issues,
        diagnostics,
    })
}

fn safe_relative(raw: &str) -> Result<()> {
    let path = Path::new(raw);
    ensure!(
        !raw.is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "旧清单含不安全路径"
    );
    ensure!(
        !raw.split(['/', '\\'])
            .any(|s| s.is_empty() || s == "." || s == ".." || s.contains(':')),
        "旧清单含不安全路径"
    );
    Ok(())
}
fn stage(
    root: &Path,
    staged: &mut BTreeMap<PathBuf, PathBuf>,
    relative: PathBuf,
    bytes: &[u8],
) -> Result<()> {
    ensure!(bytes.len() as u64 <= MAX_DOCUMENT_BYTES, "单个产物超过上限");
    let file = root.join(format!("stage-{}", staged.len()));
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)?;
    out.write_all(bytes)?;
    out.sync_all()?;
    ensure!(staged.insert(relative, file).is_none(), "重复输出路径");
    Ok(())
}
