//! Semantic message decoding and legacy search preview policy, separate from storage reads.
pub mod pages;
use super::{
    legacy,
    read::{
        LegacyReadPolicy, LegacySearch, RawMessage, Snapshot, StoredContent, MAX_DECODED_BYTES,
    },
};
use crate::business::messages::{self as domain, Conversation};
use anyhow::{ensure, Context, Result};

impl Snapshot {
    pub fn search_legacy_page(
        &self,
        stream: usize,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<RawMessage>> {
        ensure!(keyword.len() <= 4096, domain::Error::Limit);
        self.read_selection(
            stream,
            filter,
            legacy,
            limit,
            false,
            Some(LegacySearch {
                keyword,
                preview: &|raw| Ok(self.message(raw)?.preview),
            }),
        )
    }
    pub fn message(&self, raw: &RawMessage) -> Result<domain::Message> {
        self.revalidate(&raw.reference)?;
        let kind = semantic_kind(raw.local_type);
        let decoded = if matches!(raw.content, StoredContent::Null) {
            Vec::new()
        } else {
            raw.bounded_decode(MAX_DECODED_BYTES)?
        };
        let text = std::str::from_utf8(&decoded).context("invalid message text")?;
        let conversation = self.conversation(&raw.reference)?.clone();
        let is_group =
            matches!(&conversation, Conversation::Known(name) if name.ends_with("@chatroom"));
        let body = if is_group {
            crate::message::split_group_content(text).1
        } else {
            text
        };
        let mut preview =
            legacy::fmt_content(raw.local_id.unwrap_or(0), raw.local_type, text, is_group);
        let call = (kind == domain::Kind::Call).then(|| call_event(body));
        let content = match kind {
            domain::Kind::Text => domain::Content::Text(body.to_owned()),
            domain::Kind::Call => domain::Content::Call(call.clone().expect("call kind")),
            domain::Kind::System => domain::Content::System(preview.clone()),
            domain::Kind::Image | domain::Kind::Video | domain::Kind::Voice => {
                crate::adapters::wechat::structured_message::decode(raw.local_type, text, is_group)
                    .map(domain::Content::Structured)
                    .unwrap_or(domain::Content::Media(kind))
            }
            _ => {
                crate::adapters::wechat::structured_message::decode(raw.local_type, text, is_group)
                    .map(domain::Content::Structured)
                    .unwrap_or_else(domain::Content::Unavailable)
            }
        };
        if matches!(content, domain::Content::Unavailable(_))
            && body.trim_start().starts_with('<')
            && preview == body
        {
            preview = "[unavailable message preview]".into();
        }
        let sender = if is_group {
            raw.sender
                .clone()
                .filter(|name| !matches!(&conversation, Conversation::Known(chat) if name == chat))
                .or_else(|| {
                    let prefix = crate::message::split_group_content(text).0;
                    (!prefix.is_empty()).then(|| prefix.to_owned())
                })
        } else {
            raw.sender
                .clone()
                .filter(|name| !matches!(&conversation, Conversation::Known(chat) if name == chat))
        };
        Ok(domain::Message {
            reference: raw.reference.clone(),
            conversation,
            timestamp: raw.timestamp,
            sender,
            kind,
            call,
            content,
            preview,
            url: legacy::appmsg_url_for_message(raw.local_type, text),
        })
    }
}

pub fn semantic_kind(code: i64) -> domain::Kind {
    match code as u64 & 0xffff_ffff {
        1 => domain::Kind::Text,
        3 => domain::Kind::Image,
        34 => domain::Kind::Voice,
        43 | 62 => domain::Kind::Video,
        49 => domain::Kind::Structured,
        50 => domain::Kind::Call,
        10000 | 10002 => domain::Kind::System,
        _ => domain::Kind::Unknown,
    }
}

pub struct DirectoryDisplay {
    pub type_name: String,
    pub content: String,
    pub is_system: bool,
    pub structured_details: bool,
}

/// Compatibility display for directory exports. WeChat numeric codes and XML
/// interpretation stay in the adapter while the application owns rendering.
pub fn directory_display(code: i64, raw: &str, native_content: Option<&str>) -> DirectoryDisplay {
    let base = code as u64 & 0xffff_ffff;
    let xml_text = |tag: &str| {
        crate::message::xml::parse(crate::message::split_group_content(raw).1)
            .and_then(|doc| {
                doc.descendants()
                    .find(|node| node.has_tag_name(tag))
                    .and_then(|node| node.text())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| raw.chars().take(200).collect())
    };
    let (type_name, content) = match base {
        1 => ("文本".into(), raw.into()),
        3 => ("图片".into(), "[图片]".into()),
        34 => ("语音".into(), "[语音]".into()),
        42 => ("名片".into(), format!("[名片: {}]", xml_text("nickname"))),
        43 => ("视频".into(), "[视频]".into()),
        47 => ("表情包".into(), "[表情包]".into()),
        48 => ("位置".into(), format!("[位置: {}]", xml_text("label"))),
        49 => (
            "分享/文件/小程序".into(),
            format!("[分享: {}]", xml_text("title")),
        ),
        10000 => (
            "系统消息".into(),
            format!("[系统: {}]", raw.chars().take(100).collect::<String>()),
        ),
        10002 => (
            "系统通知".into(),
            format!("[系统: {}]", raw.chars().take(100).collect::<String>()),
        ),
        _ => (
            format!("未知({code})"),
            native_content
                .map(str::to_owned)
                .unwrap_or_else(|| raw.chars().take(200).collect()),
        ),
    };
    DirectoryDisplay {
        type_name,
        content,
        is_system: matches!(base, 10000 | 10002),
        structured_details: base == 49,
    }
}

pub fn call_event(content: &str) -> domain::CallEvent {
    let text = roxmltree::Document::parse(content)
        .ok()
        .and_then(|doc| {
            doc.descendants()
                .find(|node| node.has_tag_name("msg"))
                .and_then(|node| node.text())
                .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .filter(|text| !text.is_empty());
    let duration_text = text
        .as_deref()
        .and_then(|s| s.strip_prefix("Duration:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    domain::CallEvent {
        media: domain::CallMedia::Unknown,
        status_text: text,
        duration_text,
    }
}

#[cfg(test)]
mod directory_display_tests {
    use super::*;

    #[test]
    fn preserves_directory_export_labels_xml_and_unknown_fallback() {
        let card = directory_display(42, "<msg><nickname>Alice</nickname></msg>", None);
        assert_eq!(card.type_name, "名片");
        assert_eq!(card.content, "[名片: Alice]");

        let structured = directory_display(
            (7_i64 << 32) | 49,
            "<msg><title>Document</title></msg>",
            None,
        );
        assert_eq!(structured.type_name, "分享/文件/小程序");
        assert_eq!(structured.content, "[分享: Document]");
        assert!(structured.structured_details);

        let unknown = directory_display(77, "raw", Some("projected"));
        assert_eq!(unknown.type_name, "未知(77)");
        assert_eq!(unknown.content, "projected");
        assert!(!unknown.is_system);
    }
}
