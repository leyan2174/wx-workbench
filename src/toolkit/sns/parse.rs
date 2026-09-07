use super::{decode_content, sanitize_xml, Content};
use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local, Utc};
use roxmltree::Node;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 默认与旧 datetime.fromtimestamp 一样使用系统本地时区；测试/跨机导出可指定偏移。
#[derive(Clone, Copy, Debug, Default)]
pub enum TimeZone {
    #[default]
    Local,
    Fixed(FixedOffset),
}

impl TimeZone {
    pub fn format(self, seconds: i64, format: &str) -> Result<String> {
        let dt = DateTime::<Utc>::from_timestamp(seconds, 0)
            .ok_or_else(|| anyhow!("SNS timestamp out of range: {seconds}"))?;
        Ok(match self {
            Self::Local => dt.with_timezone(&Local).format(format).to_string(),
            Self::Fixed(offset) => dt.with_timezone(&offset).format(format).to_string(),
        })
    }
    pub(crate) fn display(self, seconds: i64) -> Result<String> {
        if seconds == 0 {
            Ok(String::new())
        } else {
            self.format(seconds, "%Y-%m-%d %H:%M:%S")
        }
    }
}

pub type Media = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub latitude: String,
    pub longitude: String,
    pub poi_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub create_time: Option<i64>,
    pub create_time_str: String,
    #[serde(rename = "type")]
    pub kind: Option<i64>,
    pub type_name: String,
    pub from_username: String,
    pub from_nickname: String,
    pub to_username: String,
    pub to_nickname: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub username: String,
    pub create_time: i64,
    pub create_time_str: String,
    pub content_desc: String,
    pub content_type: i64,
    pub content_type_name: String,
    pub nickname: String,
    pub is_private: bool,
    pub location: Option<Location>,
    pub media: Vec<Media>,
    /// 旧纯文本 JSON 不含媒体 ID；仅为显式缓存恢复保留 XML 身份。
    #[serde(skip)]
    pub cache_media_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<Comment>>,
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(name))
}
fn descendant<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.descendants().skip(1).find(|n| n.has_tag_name(name))
}
fn descendant_child<'a, 'i>(node: Node<'a, 'i>, parent: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.descendants()
        .skip(1)
        .filter(|n| n.has_tag_name(parent))
        .find_map(|n| child(n, name))
}
fn text(node: Node<'_, '_>, name: &str, default: &str) -> String {
    child(node, name)
        .map(|n| n.text().unwrap_or(""))
        .unwrap_or(default)
        .into()
}

pub fn timestamp_filename(seconds: i64, zone: TimeZone) -> Result<String> {
    if seconds == 0 {
        Ok("00000000000000000".into())
    } else {
        zone.format(seconds, "%Y%m%d%H%M%S000")
    }
}

/// 仅解析旧实现选中的第一个后代 TimelineObject；无该节点返回 None。
/// 非法 XML/危险声明/超限返回错误，批量读取会记录诊断并继续后续行。
pub fn parse_timeline(content: Content<'_>, zone: TimeZone) -> Result<Option<Post>> {
    let xml = sanitize_xml(&decode_content(content)?);
    if xml.is_empty() {
        return Ok(None);
    }
    let doc = roxmltree::Document::parse(&xml)?;
    let root = doc.root_element();
    let Some(tl) = descendant(root, "TimelineObject") else {
        return Ok(None);
    };
    let create_time = text(tl, "createTime", "0").trim().parse().unwrap_or(0);
    let kind = descendant_child(tl, "ContentObject", "type")
        .and_then(|n| n.text())
        .unwrap_or("0")
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    let name = match kind {
        1 => "图文",
        2 => "纯文本",
        3 => "链接",
        5 => "视频链接",
        7 => "位置",
        15 => "视频",
        28 => "短视频",
        30 => "音乐",
        34 => "笔记",
        42 => "小程序",
        54 => "直播",
        _ => "",
    };
    let location = descendant(tl, "location").and_then(|n| {
        let lat = n.attribute("latitude").unwrap_or("0");
        let lon = n.attribute("longitude").unwrap_or("0");
        (lat != "0" || lon != "0").then(|| Location {
            latitude: lat.into(),
            longitude: lon.into(),
            poi_name: n.attribute("poiName").unwrap_or("").into(),
        })
    });
    let cache_media_ids = tl
        .descendants()
        .skip(1)
        .filter(|n| n.has_tag_name("media"))
        .map(|n| text(n, "id", ""))
        .collect();
    let media = tl
        .descendants()
        .skip(1)
        .filter(|n| n.has_tag_name("media"))
        .map(|n| {
            let mut m = BTreeMap::from([
                ("type".into(), text(n, "type", "")),
                ("sub_type".into(), text(n, "sub_type", "")),
                ("video_duration".into(), text(n, "videoDuration", "0")),
            ]);
            for (tag, text_key, attrs) in [
                (
                    "thumb",
                    Some("thumb_url"),
                    vec![("key", "thumb_key"), ("token", "thumb_token")],
                ),
                (
                    "url",
                    Some("url"),
                    vec![
                        ("md5", "url_md5"),
                        ("key", "url_key"),
                        ("token", "url_token"),
                    ],
                ),
                (
                    "size",
                    None,
                    vec![
                        ("width", "width"),
                        ("height", "height"),
                        ("totalSize", "total_size"),
                    ],
                ),
            ] {
                if let Some(el) = child(n, tag) {
                    if let Some(key) = text_key {
                        m.insert(key.into(), el.text().unwrap_or("").into());
                    }
                    for (attr, key) in attrs {
                        m.insert(key.into(), el.attribute(attr).unwrap_or("").into());
                    }
                }
            }
            m
        })
        .collect();
    Ok(Some(Post {
        id: text(tl, "id", ""),
        username: text(tl, "username", ""),
        create_time,
        create_time_str: zone.display(create_time)?,
        content_desc: text(tl, "contentDesc", ""),
        content_type: kind,
        content_type_name: if name.is_empty() {
            format!("未知({kind})")
        } else {
            name.into()
        },
        nickname: descendant_child(root, "LocalExtraInfo", "nickname")
            .and_then(|n| n.text())
            .unwrap_or("")
            .into(),
        is_private: text(tl, "private", "0") == "1",
        location,
        media,
        cache_media_ids,
        tid: None,
        db_user_name: None,
        comments: None,
    }))
}
