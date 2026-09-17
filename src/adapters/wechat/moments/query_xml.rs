pub(crate) use super::super::xml_fragments::element_text as extract_xml_text;
use super::super::xml_fragments::unescape_entities as unescape_html;
use roxmltree::{Document, Node};
use serde_json::Value;

#[cfg(test)]
#[path = "query_xml_tests.rs"]
mod tests;

fn extract_xml_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let start = xml.find(&open)?;
    let tag_end = start + xml[start..].find('>')?;
    let attr_pat = format!(r#"{}=""#, attr);
    let attr_start = start + xml[start..tag_end].find(&attr_pat)? + attr_pat.len();
    let attr_end = attr_start + xml[attr_start..tag_end].find('"')?;
    let value = xml[attr_start..attr_end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn escape_like_pattern(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_")
}

fn xml_child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.has_tag_name(tag))
}

fn xml_text<'a, 'input>(node: Option<Node<'a, 'input>>) -> Option<String> {
    node.and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn xml_attr<'a, 'input>(node: Option<Node<'a, 'input>>, attr: &str) -> Option<String> {
    node.and_then(|n| n.attribute(attr))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn insert_media_string(out: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::String(value));
    }
}

fn insert_media_i64(out: &mut serde_json::Map<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::from(value));
    }
}

/// 从已经定位到的 `<TimelineObject>` 节点里抽 `<mediaList>/<media>` 数组。
/// 字段名与 artifacts 仓库 `wechat_sns_dump.py::_parse_media` 对齐，
/// 便于跨实现 diff。缺失字段直接省略（不输出 null），供下游代理图片 / 离线渲染。
fn parse_media_from_timeline(timeline: Node) -> Vec<Value> {
    let Some(media_list) =
        xml_child(timeline, "ContentObject").and_then(|node| xml_child(node, "mediaList"))
    else {
        return Vec::new();
    };

    media_list
        .children()
        .filter(|node| node.is_element() && node.has_tag_name("media"))
        .map(|media| {
            let url_el = xml_child(media, "url");
            let thumb_el = xml_child(media, "thumb");
            let size_el = xml_child(media, "size");
            let enc_el = xml_child(media, "enc");
            let mut out = serde_json::Map::new();

            insert_media_string(&mut out, "id", xml_text(xml_child(media, "id")));
            insert_media_string(&mut out, "type", xml_text(xml_child(media, "type")));
            insert_media_string(&mut out, "sub_type", xml_text(xml_child(media, "sub_type")));
            insert_media_string(&mut out, "url", xml_text(url_el));
            insert_media_string(&mut out, "thumb", xml_text(thumb_el));
            insert_media_string(&mut out, "md5", xml_attr(url_el, "md5"));
            insert_media_string(&mut out, "url_key", xml_attr(url_el, "key"));
            insert_media_string(&mut out, "url_token", xml_attr(url_el, "token"));
            insert_media_string(&mut out, "url_enc_idx", xml_attr(url_el, "enc_idx"));
            insert_media_string(&mut out, "thumb_key", xml_attr(thumb_el, "key"));
            insert_media_string(&mut out, "thumb_token", xml_attr(thumb_el, "token"));
            insert_media_string(&mut out, "thumb_enc_idx", xml_attr(thumb_el, "enc_idx"));
            insert_media_string(&mut out, "enc_key", xml_attr(enc_el, "key"));
            insert_media_i64(
                &mut out,
                "width",
                xml_attr(size_el, "width").and_then(|v| v.parse::<i64>().ok()),
            );
            insert_media_i64(
                &mut out,
                "height",
                xml_attr(size_el, "height").and_then(|v| v.parse::<i64>().ok()),
            );
            insert_media_i64(
                &mut out,
                "total_size",
                xml_attr(size_el, "totalSize").and_then(|v| v.parse::<i64>().ok()),
            );
            insert_media_string(
                &mut out,
                "video_md5",
                xml_text(xml_child(media, "videomd5")).or_else(|| xml_attr(url_el, "videomd5")),
            );
            insert_media_i64(
                &mut out,
                "video_duration",
                xml_text(xml_child(media, "videoDuration")).and_then(|v| v.parse::<i64>().ok()),
            );

            Value::Object(out)
        })
        .collect()
}

/// 从 `SnsTimeLine.content` 整段 XML 抽 media[]。仅供单测使用 —— 生产路径走
/// `parse_post_xml`，那边已经把整份 doc parse 一次直接复用 timeline 节点。
#[cfg(test)]
pub(crate) fn parse_post_media(xml: &str) -> Vec<Value> {
    let Ok(doc) = Document::parse(xml) else {
        return Vec::new();
    };
    let Some(timeline) = doc.descendants().find(|n| n.has_tag_name("TimelineObject")) else {
        return Vec::new();
    };
    parse_media_from_timeline(timeline)
}

/// SnsTimeLine 行解析产物。不含 display name（依赖 Names，需要出 spawn_blocking 再补）。
#[derive(Clone, Debug)]
pub(crate) struct ParsedPost {
    pub author: super::Author,
    pub recovered: bool,
    pub tid: i64,
    pub post_id: String,
    pub create_time: i64,
    pub author_username: String,
    pub content: String,
    pub media: Vec<Value>,
    pub location: String,
}

fn parse_post_xml_fallback(tid: i64, user_name_column: &str, content: &str) -> ParsedPost {
    let create_time = extract_xml_text(content, "createTime")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let text = extract_xml_text(content, "contentDesc")
        .map(|s| unescape_html(&s))
        .unwrap_or_default();
    let embedded = extract_xml_text(content, "username")
        .map(|s| unescape_html(&s))
        .unwrap_or_default();
    let author = super::author(user_name_column, &embedded);
    let author_username = author.effective.clone().unwrap_or_default();
    let location = extract_xml_attr(content, "location", "poiName")
        .map(|s| unescape_html(&s))
        .unwrap_or_default();

    ParsedPost {
        author,
        recovered: true,
        tid,
        post_id: extract_xml_text(content, "id").unwrap_or_default(),
        create_time,
        author_username,
        content: text,
        media: Vec::new(),
        location,
    }
}

/// 纯 XML 解析，无 Names 依赖，可以在 spawn_blocking 里跑。
/// user_name_column 为空时从 TimelineObject/<username> 兜底（转发帖）。
///
/// 单 roxmltree DOM 解析一次出全部字段（createTime / contentDesc / username / media / location），
/// 取代旧版 regex + DOM 双解析。XML entity 解码（`&lt;` / `&amp;` 等）由 roxmltree 自动处理，
/// 旧版 `extract_xml_text` 是字符串扫描不解码 —— 因此 `content` / `location` / `username` 字段
/// 现在会输出解码后的文本，对下游是更正确的语义。
/// 如果 XML 已损坏到无法 DOM parse，或缺少 `TimelineObject`，则退回轻量 string
/// fallback，尽量保住 createTime / contentDesc / username / location，避免一条帖子
/// 因为局部坏 XML 被整体打成零值，影响排序 / 搜索 / 作者过滤语义。
pub(crate) fn parse_post_xml(tid: i64, user_name_column: &str, content: &str) -> ParsedPost {
    let Ok(doc) = Document::parse(content) else {
        return parse_post_xml_fallback(tid, user_name_column, content);
    };
    let Some(timeline) = doc.descendants().find(|n| n.has_tag_name("TimelineObject")) else {
        return parse_post_xml_fallback(tid, user_name_column, content);
    };

    let create_time = xml_text(xml_child(timeline, "createTime"))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let text = xml_text(xml_child(timeline, "contentDesc")).unwrap_or_default();
    let post_id = xml_text(xml_child(timeline, "id")).unwrap_or_default();
    let embedded = xml_text(xml_child(timeline, "username")).unwrap_or_default();
    let author = super::author(user_name_column, &embedded);
    let author_username = author.effective.clone().unwrap_or_default();
    let media = parse_media_from_timeline(timeline);
    let location = xml_child(timeline, "location")
        .and_then(|n| n.attribute("poiName"))
        .map(str::to_string)
        .unwrap_or_default();

    ParsedPost {
        author,
        recovered: false,
        tid,
        post_id,
        create_time,
        author_username,
        content: text,
        media,
        location,
    }
}
