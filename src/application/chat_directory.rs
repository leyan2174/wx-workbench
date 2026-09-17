//! 聊天目录导出用例；媒体准备、渲染和发布分别处理，不自行查询消息库。
mod media;
mod render;

use crate::infrastructure::output_tree::{self as publish, Binding, ExistingPolicy};
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

#[cfg(test)]
mod raw_voice_tests {
    use super::*;

    #[test]
    fn default_media_chat_export_keeps_raw_silk_and_machine_reference() {
        let root = tempfile::tempdir().unwrap();
        let account = root.path().join("account");
        let decrypted = account.join("decrypted");
        fs::create_dir_all(decrypted.join("message")).unwrap();
        let runtime = RuntimeContext {
            config: crate::config::Config {
                key_store: None,
                db_dir: account.join("db_storage"),
                keys_file: account.join("keys.json"),
                decrypted_dir: decrypted.clone(),
                wechat_process: String::new(),
            },
            config_path: account.join("config.json"),
            root: account.clone(),
            id: "synthetic".into(),
            directory: account.join("runtime"),
        };
        let target = Target {
            username: "peer".into(),
            chat: "Peer".into(),
            is_group: false,
        };
        let conn = rusqlite::Connection::open(decrypted.join("message/message_0.db")).unwrap();
        let table = format!("Msg_{:x}", md5::compute("peer"));
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER); INSERT INTO [{table}] VALUES(7,34,123,987);")).unwrap();
        drop(conn);
        let raw = b"\x02#!SILK_V3\0\x01synthetic\xff";
        let conn = rusqlite::Connection::open(decrypted.join("message/media_0.db")).unwrap();
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(91,'peer'); CREATE TABLE VoiceInfo(chat_name_id INTEGER, local_id INTEGER, create_time INTEGER, svr_id INTEGER, voice_data BLOB);").unwrap();
        conn.execute(
            "INSERT INTO VoiceInfo VALUES(91,700,123,987,?1)",
            [raw.as_slice()],
        )
        .unwrap();
        drop(conn);
        let document = json!({"username":"peer", "is_group":false, "messages":[{
            "source":"message/message_0.db", "local_id":7, "local_type":34,
            "server_id":987, "sort_seq":1, "status":null, "sender_username":null,
            "sender":"", "timestamp":123, "raw_content":"<msg><voicemsg voicelength='1250'/></msg>"
        }]});
        let options = Options {
            formats: [Format::Json, Format::Html].into(),
            media_enabled: true,
            update: false,
            max_media_bytes: 1024 * 1024,
            max_total_media_bytes: 1024 * 1024,
        };
        let output = root.path().join("out");
        let report = export_document(
            &runtime,
            &json!({}),
            &target,
            &document,
            &output,
            &options,
            MediaInput::current(None),
        )
        .unwrap();
        assert_eq!(report.media_issues, 0);
        let manifest: Value =
            serde_json::from_slice(&fs::read(output.join("_voice_manifest.json")).unwrap())
                .unwrap();
        let item = &manifest["items"][0];
        assert_eq!(item["status"], "success");
        assert_eq!(item["association"], "exact_message_media_join");
        assert!(item["message_id"].is_string());
        assert!(item["sender"].is_null());
        assert_eq!(item["duration_ms"], 1250);
        assert_eq!(item["evidence"]["media"]["media_rowid"], 1);
        let relative = item["relative_path"].as_str().unwrap();
        assert!(relative.starts_with("voice/") && relative.ends_with(".silk"));
        assert_eq!(fs::read(output.join(relative)).unwrap(), raw);
        let html = fs::read_to_string(output.join("message_0.db.html")).unwrap();
        assert!(html.contains(relative));
        assert!(!html.contains("<audio"));
    }
}

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
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
}

#[derive(Clone, Copy)]
pub struct MediaInput<'a> {
    pub(super) sources: Option<&'a [crate::adapters::wechat::media::voice::DecryptedSource]>,
    pub(super) image_material: Option<([u8; 16], u8)>,
}

impl<'a> MediaInput<'a> {
    pub fn current(image_material: Option<([u8; 16], u8)>) -> Self {
        Self {
            sources: None,
            image_material,
        }
    }

    pub fn snapshot(
        sources: &'a [crate::adapters::wechat::media::voice::DecryptedSource],
        image_material: Option<([u8; 16], u8)>,
    ) -> Self {
        Self {
            sources: Some(sources),
            image_material,
        }
    }
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
    #[serde(skip)]
    pub structured_details: bool,
    pub is_received: bool,
    pub source: String,
    pub media: Vec<Media>,
    /// 保留已有核心解析出的卡片、引用等扩展信息。
    pub native: Value,
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value.get(key).with_context(|| {
        format!("ExportDirectoryByUsername 缺少目录契约字段 {key}；拒绝生成不完整输出")
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
        let display = crate::adapters::wechat::messages::directory_display(
            local_type,
            raw.as_str().unwrap_or(""),
            value.get("content").and_then(Value::as_str),
        );
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
            type_name: display.type_name,
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
            display_content: display.content,
            is_system: display.is_system,
            structured_details: display.structured_details,
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
                crate::infrastructure::publication::resolved(&actual)?
                    .to_string_lossy()
                    .eq_ignore_ascii_case(
                        &crate::infrastructure::publication::resolved(expected)?.to_string_lossy(),
                    ),
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
    media: MediaInput<'_>,
) -> Result<Report> {
    export_document_impl(runtime, config, target, document, output, options, media)
}

#[derive(Serialize)]
struct VoiceManifestEvidence {
    message_source: String,
    message_local_id: i64,
    server_id: Value,
    item_index: usize,
    media: Value,
}

fn voice_manifest(
    runtime_id: &str,
    target: &Target,
    rows: &[Row],
) -> Vec<crate::business::voice_export::ManifestItem<VoiceManifestEvidence>> {
    use crate::business::voice_export::{stable_message_id, ManifestItem};
    rows.iter()
        .flat_map(|row| {
            row.media
                .iter()
                .filter(|media| media.kind == "voice")
                .enumerate()
                .map(move |(index, media)| {
                    let server = row
                        .server_id
                        .as_i64()
                        .or_else(|| row.server_id.as_str().and_then(|s| s.parse().ok()));
                    let direct = row.local_type & 0xffff_ffff == 34;
                    ManifestItem {
                        account_id: runtime_id.to_owned(),
                        message_id: stable_message_id(&target.username, server),
                        conversation: Some(target.username.clone()),
                        sender: (!row.sender_username.is_empty())
                            .then(|| row.sender_username.clone()),
                        timestamp: row.create_time,
                        duration_ms: if direct {
                            row.content
                                .as_str()
                                .and_then(crate::adapters::wechat::media::voice::voice_duration_ms)
                        } else {
                            None
                        },
                        encoding: media.path.as_ref().map(|_| "silk".into()),
                        relative_path: media.path.clone(),
                        status: if media.status == "available" {
                            "success".into()
                        } else {
                            media.status.clone()
                        },
                        association: media.binding.clone().unwrap_or_else(|| "unproven".into()),
                        evidence: VoiceManifestEvidence {
                            message_source: row.source.clone(),
                            message_local_id: row.local_id,
                            server_id: row.server_id.clone(),
                            item_index: index,
                            media: media.evidence.clone(),
                        },
                        failure: (!matches!(media.status.as_str(), "available" | "disabled"))
                            .then(|| media.detail.clone()),
                    }
                })
        })
        .collect()
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
    media: MediaInput<'_>,
) -> Result<Report> {
    ensure!(media.sources.is_some(), "静态媒体导出缺少来源清单");
    export_document_impl(runtime, config, target, document, output, options, media)
}

fn export_document_impl(
    runtime: &RuntimeContext,
    config: &Value,
    target: &Target,
    document: &Value,
    output: &Path,
    options: &Options,
    media: MediaInput<'_>,
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
    let mut protected = crate::infrastructure::publication::export_protected(runtime);
    let inputs = media::Inputs::from_config(runtime, config, media, options.media_enabled)?;
    protected.extend(inputs.paths());
    crate::infrastructure::publication::validate_export_target(runtime, output)?;
    for path in &protected {
        crate::infrastructure::publication::separate(path, output)?;
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
    stage(
        staging.path(),
        &mut staged,
        PathBuf::from("_voice_manifest.json"),
        &serde_json::to_vec_pretty(
            &json!({"version": 1, "items": voice_manifest(&runtime.id, target, &rows)}),
        )?,
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
    crate::infrastructure::publication::validate_export_target(runtime, output)?;
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
    crate::infrastructure::publication::validate_export_target(runtime, output)?;
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
