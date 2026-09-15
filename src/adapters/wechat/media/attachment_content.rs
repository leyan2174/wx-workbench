//! WeChat attachment decoding and cache naming. No filesystem access or authorization.
//! MessageInput/Identity/AttachmentMetadata preserve the explicit legacy evidence projection.
use crate::business::attachment_content::{
    AttachmentContent, ContainerKind, NamedKind, NamedMedia,
};
use crate::message::{split_group_content, xml};
use roxmltree::{Document, Node};
use serde::Serialize;
use std::path::{Path, PathBuf};
const RECORD_XML_LIMIT: usize = 500_000;

/// Decoding of source message flags is adapter-owned, not a protocol enum method.
pub(crate) fn message_kind(raw_type: i64) -> Option<Kind> {
    match raw_type & 0xffff_ffff {
        3 => Some(Kind::Image),
        34 => Some(Kind::Voice),
        43 => Some(Kind::Video),
        47 => Some(Kind::Emoticon),
        49 => Some(Kind::File),
        _ => None,
    }
}

/// Historical export classification only. The selected container is subsequently
/// decoded by the strict file/record parser; this does not authorize attachment IO.
pub(crate) fn legacy_container_kind(body: &str) -> Option<ContainerKind> {
    let body = split_group_content(body).1;
    let doc = xml::parse(body)?;
    let app = doc.descendants().find(|node| node.has_tag_name("appmsg"))?;
    let value: i64 = app
        .children()
        .find(|node| node.has_tag_name("type"))?
        .text()?
        .trim()
        .parse()
        .ok()?;
    match value {
        6 => Some(ContainerKind::File),
        19 => Some(ContainerKind::Record),
        _ => None,
    }
}

/// Strict message-bound digest: duplicate nodes/attributes and invalid hashes
/// are failures, never permission to scan for a likely filename.
pub(crate) fn named_media(body: &str, kind: NamedKind) -> Result<NamedMedia> {
    let doc = parse_xml(split_group_content(body).1, false)?;
    let tag = match kind {
        NamedKind::Video => "videomsg",
        NamedKind::Emoticon => "emoji",
    };
    let node = unique(doc.descendants().filter(|node| named(*node, tag)))?
        .ok_or_else(|| error(ErrorKind::WrongType, "missing supported media node"))?;
    let digest = hash(node.attribute("md5").unwrap_or(""))?
        .ok_or_else(|| error(ErrorKind::InvalidHash, "missing media digest"))?;
    Ok(NamedMedia { kind, digest })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidIdentity,
    InvalidXml,
    InvalidMetadata,
    InvalidHash,
    WrongType,
    NotLoaded,
    InvalidIndex,
    UnsafePath,
    Ambiguous,
    HashMismatch,
    LimitExceeded,
    Changed,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub stage: &'static str,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.stage)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn error(kind: ErrorKind, stage: &'static str) -> Error {
    Error { kind, stage }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageInput<'a> {
    pub username: &'a str,
    pub source: &'a str,
    pub local_id: i64,
    pub create_time: i64,
    pub body: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub username: String,
    pub source: String,
    pub local_id: i64,
    pub create_time: i64,
}

pub use crate::business::attachment_content::Kind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentMetadata {
    pub identity: Identity,
    pub datatype: Option<String>,
    #[serde(flatten)]
    pub content: AttachmentContent,
}

impl std::ops::Deref for AttachmentMetadata {
    type Target = AttachmentContent;
    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

impl std::ops::DerefMut for AttachmentMetadata {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.content
    }
}

pub(crate) fn identity(input: &MessageInput<'_>) -> Result<Identity> {
    let source = input.source.replace('\\', "/");
    let parts: Vec<_> = source.split('/').collect();
    let valid_source = parts.len() == 2
        && parts[0].eq_ignore_ascii_case("message")
        && parts[1]
            .to_ascii_lowercase()
            .strip_prefix("message_")
            .and_then(|s| s.strip_suffix(".db"))
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    if input.username.is_empty()
        || input.username.len() > 1024
        || input.username.chars().any(char::is_control)
        || input.local_id <= 0
        || input.source.len() > 1024
        || !valid_source
    {
        return Err(error(
            ErrorKind::InvalidIdentity,
            "需要精确 username、完整消息来源与正 local_id",
        ));
    }
    Ok(Identity {
        username: input.username.into(),
        source,
        local_id: input.local_id,
        create_time: input.create_time,
    })
}

fn child<'a, 'i>(node: Node<'a, 'i>, tag: &str) -> Result<Option<Node<'a, 'i>>> {
    unique(node.children().filter(|n| named(*n, tag)))
}
fn named(node: Node<'_, '_>, tag: &str) -> bool {
    node.has_tag_name(tag) && node.tag_name().namespace().is_none()
}
fn unique<'a, 'i>(mut nodes: impl Iterator<Item = Node<'a, 'i>>) -> Result<Option<Node<'a, 'i>>> {
    let first = nodes.next();
    if nodes.next().is_some() {
        return Err(error(ErrorKind::InvalidMetadata, "同名 XML 节点不唯一"));
    }
    Ok(first)
}
fn text<'a, 'i>(node: Node<'a, 'i>, tag: &str) -> Result<&'a str> {
    Ok(child(node, tag)?.and_then(|n| n.text()).unwrap_or(""))
}
fn scalar_text(node: Option<Node<'_, '_>>) -> Result<String> {
    let Some(node) = node else {
        return Ok(String::new());
    };
    if node.children().any(|n| n.is_element()) {
        return Err(error(ErrorKind::InvalidMetadata, "标量字段不能包含子元素"));
    }
    // 合并所有文本及 CDATA 片段，包括被 XML 注释分隔的文本。
    Ok(node
        .children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect())
}
fn descendant_text<'a, 'i>(node: Node<'a, 'i>, tag: &str) -> Result<&'a str> {
    Ok(
        unique(node.descendants().skip(1).filter(|n| named(*n, tag)))?
            .and_then(|n| n.text())
            .unwrap_or(""),
    )
}

fn parse_xml(body: &str, large_record: bool) -> Result<Document<'_>> {
    if let Some(doc) = xml::parse(body) {
        return Ok(doc);
    }
    // 待共享入口支持显式 limit 后合并；只为记录开放 500K，不扩大普通文件 XML 上限。
    if large_record && body.chars().take(RECORD_XML_LIMIT + 1).count() <= RECORD_XML_LIMIT {
        let upper = body.to_ascii_uppercase();
        if !upper.contains("<!DOCTYPE") && !upper.contains("<!ENTITY") {
            return Document::parse(body)
                .map_err(|_| error(ErrorKind::InvalidXml, "记录 XML 无效"));
        }
    }
    Err(error(
        ErrorKind::InvalidXml,
        "XML 无效、超限或包含 DTD/实体声明",
    ))
}
fn body<'a>(input: &MessageInput<'a>) -> &'a str {
    if input.username.ends_with("@chatroom") {
        split_group_content(input.body).1
    } else {
        input.body
    }
}
fn appmsg<'a, 'i>(doc: &'a Document<'i>, expected_type: u64) -> Result<Node<'a, 'i>> {
    let node = unique(
        doc.root_element()
            .descendants()
            .skip(1)
            .filter(|n| named(*n, "appmsg")),
    )?
    .ok_or_else(|| error(ErrorKind::InvalidMetadata, "缺少 appmsg"))?;
    if number(text(node, "type")?)? != Some(expected_type) {
        return Err(error(ErrorKind::WrongType, "appmsg type 不匹配"));
    }
    Ok(node)
}
fn number(raw: &str) -> Result<Option<u64>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    if raw
        .split('_')
        .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(error(
            ErrorKind::InvalidMetadata,
            "大小或类型必须是非负整数",
        ));
    }
    let value: u64 = raw
        .replace('_', "")
        .parse()
        .map_err(|_| error(ErrorKind::InvalidMetadata, "整数超限"))?;
    Ok((value != 0).then_some(value))
}
pub(crate) fn hash(raw: &str) -> Result<Option<String>> {
    let value = xml::collapse(raw);
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() != 32 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(error(ErrorKind::InvalidHash, "无效 MD5 不能降级为无 hash"));
    }
    Ok(Some(value.to_ascii_lowercase()))
}
pub(crate) fn safe_name(name: &str) -> Result<()> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|s| {
            s.chars().count() == 1 && s.chars().all(|c| "0123456789¹²³".contains(c))
        })
    });
    if name.is_empty()
        || name.encode_utf16().count() > 255
        || name.ends_with(['.', ' '])
        || device
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(error(
            ErrorKind::UnsafePath,
            "文件名包含路径、ADS、设备名或非法尾部",
        ));
    }
    Ok(())
}
fn title(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    // 先检查原值，不能通过 trim/collapse 隐藏尾部空格或控制字符。
    safe_name(raw)?;
    let value = xml::collapse(raw);
    safe_name(&value)?;
    Ok(value)
}

pub fn parse_file_message(input: &MessageInput<'_>) -> Result<AttachmentMetadata> {
    let identity = identity(input)?;
    let doc = parse_xml(body(input), false)?;
    let app = appmsg(&doc, 6)?;
    child(app, "appattach")?
        .ok_or_else(|| error(ErrorKind::InvalidMetadata, "文件缺少 appattach"))?;
    let title = title(text(app, "title")?)?;
    if title.is_empty() {
        return Err(error(ErrorKind::InvalidMetadata, "文件缺少 title"));
    }
    Ok(AttachmentMetadata {
        identity,
        datatype: None,
        content: AttachmentContent {
            kind: Kind::File,
            item_index: None,
            item_count: None,
            title,
            extension: xml::collapse(descendant_text(app, "fileext")?),
            expected_size: number(&scalar_text(unique(
                app.descendants().skip(1).filter(|n| named(*n, "totallen")),
            )?)?)?,
            expected_md5: hash(&scalar_text(child(app, "md5")?)?)?,
            sender: String::new(),
            description: String::new(),
        },
    })
}

pub fn parse_record_item(input: &MessageInput<'_>, item_index: i64) -> Result<AttachmentMetadata> {
    let identity = identity(input)?;
    let index = usize::try_from(item_index)
        .map_err(|_| error(ErrorKind::InvalidIndex, "item_index 不能为负数"))?;
    let body = body(input);
    let doc = parse_xml(body, body.contains("<type>19</type>"))?;
    let app = appmsg(&doc, 19)?;
    let inner = text(app, "recorditem")?;
    if inner.is_empty() {
        return Err(error(ErrorKind::NotLoaded, "recorditem 尚未加载"));
    }
    let record = parse_xml(inner, true)?;
    let list = child(record.root_element(), "datalist")?
        .ok_or_else(|| error(ErrorKind::NotLoaded, "datalist 尚未加载"))?;
    // 仅直接 dataitem，计数不受 history 的 10/50 条展示裁剪影响，不展开嵌套记录。
    let mut count = 0;
    let mut selected = None;
    for item in list.children().filter(|n| named(*n, "dataitem")) {
        if count == index {
            selected = Some(item);
        }
        count += 1;
    }
    if count == 0 {
        return Err(error(ErrorKind::NotLoaded, "datalist 为空"));
    }
    let item = selected.ok_or_else(|| error(ErrorKind::InvalidIndex, "item_index 超出范围"))?;
    let datatype = item.attribute("datatype").unwrap_or("").trim();
    let kind = match datatype {
        "1" => Kind::Text,
        "2" => Kind::Image,
        "4" => Kind::Voice,
        "5" => Kind::Video,
        "8" => Kind::File,
        _ => Kind::MetadataOnly,
    };
    Ok(AttachmentMetadata {
        identity,
        datatype: Some(datatype.into()),
        content: AttachmentContent {
            kind,
            item_index: Some(index),
            item_count: Some(count),
            title: title(text(item, "datatitle")?)?,
            extension: xml::collapse(text(item, "datafmt")?),
            expected_size: number(&scalar_text(child(item, "datasize")?)?)?,
            expected_md5: hash(&scalar_text(child(item, "fullmd5")?)?)?,
            sender: xml::collapse(text(item, "sourcename")?),
            description: xml::collapse(text(item, "datadesc")?),
        },
    })
}

pub(crate) fn file_match(name: &str, title: &str) -> bool {
    if name == title {
        return true;
    }
    let split = title.rfind('.').filter(|i| *i > 0).unwrap_or(title.len());
    let (stem, ext) = title.split_at(split);
    name.strip_prefix(stem)
        .and_then(|s| s.strip_suffix(ext))
        .map(|s| s.strip_prefix(' ').unwrap_or(s))
        .and_then(|s| s.strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
pub(crate) fn image_match(name: &str, index: usize) -> bool {
    let index = index.to_string();
    name == index
        || name
            .strip_prefix(&index)
            .is_some_and(|s| s.starts_with(['.', '_']))
}

pub(crate) fn validate_metadata(meta: &AttachmentMetadata) -> Result<Option<String>> {
    identity(&MessageInput {
        username: &meta.identity.username,
        source: &meta.identity.source,
        local_id: meta.identity.local_id,
        create_time: meta.identity.create_time,
        body: "",
    })?;
    if !meta.title.is_empty() {
        safe_name(&meta.title)?;
    }
    hash(meta.expected_md5.as_deref().unwrap_or(""))
}

/// Each ancestor must be pinned by the host, in order, before enumeration.
pub(crate) fn cache_ancestors(base: &Path, meta: &AttachmentMetadata) -> Vec<PathBuf> {
    let msg = base.join("msg");
    if meta.item_index.is_some() {
        let attach = msg.join("attach");
        let root = attach.join(format!(
            "{:x}",
            md5::compute(meta.identity.username.as_bytes())
        ));
        vec![msg, attach, root]
    } else {
        vec![msg.clone(), msg.join("file")]
    }
}

pub(crate) fn record_collection(month: &Path) -> PathBuf {
    month.join("Rec")
}

pub(crate) fn record_media(card: &Path, kind: Kind) -> Option<PathBuf> {
    let sub = match kind {
        Kind::Image => "Img",
        Kind::Voice => "A",
        Kind::Video => "V",
        Kind::File => "F",
        _ => return None,
    };
    Some(card.join(sub))
}

pub(crate) fn record_item_directory(media: &Path, kind: Kind, index: usize) -> PathBuf {
    if kind == Kind::Image {
        media.to_owned()
    } else {
        media.join(index.to_string())
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    const DIGEST: &str = "900150983cd24fb0d6963f7d28e17f72";

    #[test]
    fn source_flags_are_decoded_without_changing_protocol_kinds() {
        for (raw, kind) in [
            (3, Kind::Image),
            (34, Kind::Voice),
            (43, Kind::Video),
            (47, Kind::Emoticon),
            (49, Kind::File),
        ] {
            assert_eq!(message_kind(raw), Some(kind));
            assert_eq!(
                message_kind(((0xDEAD_BEEFu64 << 32) as i64) | raw),
                Some(kind)
            );
        }
        assert_eq!(message_kind(999), None);
    }

    #[test]
    fn named_content_is_typed_and_normalizes_digest() {
        for (tag, kind) in [
            ("videomsg", NamedKind::Video),
            ("emoji", NamedKind::Emoticon),
        ] {
            let raw = format!("<msg><{tag} md5='{}'/></msg>", DIGEST.to_uppercase());
            assert_eq!(
                named_media(&raw, kind).unwrap(),
                NamedMedia {
                    kind,
                    digest: DIGEST.into(),
                }
            );
        }
    }

    #[test]
    fn strict_named_content_rejects_unknown_missing_ambiguous_and_unsafe_xml() {
        let valid = format!("<videomsg md5='{DIGEST}'/>");
        for (raw, expected) in [
            ("<msg><unknown/></msg>".to_owned(), ErrorKind::WrongType),
            ("<msg><videomsg/></msg>".to_owned(), ErrorKind::InvalidHash),
            (
                "<msg><videomsg md5='bad'/></msg>".to_owned(),
                ErrorKind::InvalidHash,
            ),
            (
                format!("<msg>{valid}{valid}</msg>"),
                ErrorKind::InvalidMetadata,
            ),
            (
                format!("<msg><videomsg xmlns='foreign' md5='{DIGEST}'/></msg>"),
                ErrorKind::WrongType,
            ),
            (
                format!("<!DOCTYPE msg><msg>{valid}</msg>"),
                ErrorKind::InvalidXml,
            ),
            ("<msg>".to_owned(), ErrorKind::InvalidXml),
        ] {
            assert_eq!(
                named_media(&raw, NamedKind::Video).unwrap_err().kind,
                expected
            );
        }
        let oversized = format!("<msg>{valid}{}{}</msg>", " ".repeat(20_001), valid);
        assert_eq!(
            named_media(&oversized, NamedKind::Video).unwrap_err().kind,
            ErrorKind::InvalidXml
        );
    }

    #[test]
    fn legacy_classification_does_not_weaken_strict_container_decode() {
        let raw =
            "<msg><appmsg><type>6</type><type>19</type><title>a</title><appattach/></appmsg></msg>";
        assert_eq!(legacy_container_kind(raw), Some(ContainerKind::File));
        let input = MessageInput {
            username: "synthetic",
            source: "message/message_0.db",
            local_id: 1,
            create_time: 123,
            body: raw,
        };
        assert_eq!(
            parse_file_message(&input).unwrap_err().kind,
            ErrorKind::InvalidMetadata
        );
        assert_eq!(
            legacy_container_kind("<msg><appmsg><type>999</type></appmsg></msg>"),
            None
        );
    }

    #[test]
    fn decoded_content_and_legacy_projection_have_separate_contracts() {
        let input = MessageInput {
            username: "synthetic",
            source: "message/message_0.db",
            local_id: 1,
            create_time: 123,
            body: "<msg><appmsg><type>6</type><title>a.txt</title><appattach/></appmsg></msg>",
        };
        let metadata = parse_file_message(&input).unwrap();
        let content = serde_json::to_value(&metadata.content).unwrap();
        for key in ["identity", "datatype", "source", "local_id"] {
            assert!(content.get(key).is_none());
        }
        let legacy = serde_json::to_value(&metadata).unwrap();
        assert_eq!(legacy["identity"]["local_id"], 1);
        assert_eq!(legacy["title"], "a.txt");
        assert_eq!(legacy["kind"], "file");
        assert!(legacy.get("datatype").unwrap().is_null());
        assert!(legacy.get("content").is_none());
    }
}
