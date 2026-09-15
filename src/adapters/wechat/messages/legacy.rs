//! Legacy display projection for typed WeChat message decoding.
use crate::adapters::wechat::legacy_text::{
    element_text as extract_xml_text, strip_cdata as strip_xml_cdata,
    unescape_entities as unescape_html,
};
use roxmltree::{Document, Node};
fn strip_group_prefix(s: &str) -> String {
    crate::message::split_group_content(s).1.to_owned()
}
fn xml_child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(tag))
}
fn xml_text(node: Option<Node<'_, '_>>) -> Option<String> {
    node.and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(crate) fn fmt_type(t: i64) -> String {
    let base = (t as u64 & 0xFFFFFFFF) as i64;
    match base {
        1 => "文本".into(),
        3 => "图片".into(),
        34 => "语音".into(),
        42 => "名片".into(),
        43 => "视频".into(),
        47 => "表情".into(),
        48 => "位置".into(),
        49 => "链接/文件".into(),
        50 => "通话".into(),
        10000 => "系统".into(),
        10002 => "撤回".into(),
        _ => format!("type={}", base),
    }
}

pub(crate) fn fmt_content(local_id: i64, local_type: i64, content: &str, is_group: bool) -> String {
    let base = (local_type as u64 & 0xFFFFFFFF) as i64;
    match base {
        3 => return format!("[图片] local_id={}", local_id),
        47 => return "[表情]".into(),
        10000 => return parse_sysmsg(content).unwrap_or_else(|| "[系统消息]".into()),
        10002 => return parse_revoke(content).unwrap_or_else(|| "[撤回了一条消息]".into()),
        _ => {}
    }

    let text = if is_group {
        crate::message::split_group_content(content).1
    } else {
        content
    };

    match base {
        34 => return super::summary::voice(text),
        43 => return super::summary::video(text),
        50 => return super::summary::voip(text).unwrap_or_else(|| "[通话]".into()),
        42 => return super::summary::namecard(text).unwrap_or_else(|| "[名片]".into()),
        48 => return super::summary::location(text).unwrap_or_else(|| "[位置]".into()),
        _ => {}
    }

    if base == 49 && text.contains("<appmsg") {
        if let Some(parsed) = parse_appmsg(text) {
            return parsed;
        }
    }
    text.to_string()
}

/// 解析撤回消息 XML，提取被撤回的内容摘要
/// `<sysmsg type="revokemsg"><revokemsg><content>...</content></revokemsg></sysmsg>`
pub(crate) fn parse_revoke(xml: &str) -> Option<String> {
    let inner = extract_xml_text(xml, "content")?;
    // 有时 content 是 "xxx recalled a message" 英文，有时是中文
    if inner.is_empty() {
        return Some("[撤回了一条消息]".into());
    }
    // 尝试简化：如果是 XML 格式的撤回内容，直接显示摘要
    Some(format!(
        "[撤回] {}",
        inner.chars().take(30).collect::<String>()
    ))
}

/// 解析系统消息 XML（群通知等）
pub(crate) fn parse_sysmsg(xml: &str) -> Option<String> {
    // 常见格式：<sysmsg type="...">...</sysmsg>
    // 尝试提取 content 标签
    if let Some(s) = extract_xml_text(xml, "content") {
        if !s.is_empty() {
            return Some(format!("[系统] {}", s.chars().take(50).collect::<String>()));
        }
    }
    // 纯文本系统消息（无 XML）
    if !xml.starts_with('<') {
        return Some(format!(
            "[系统] {}",
            xml.chars().take(50).collect::<String>()
        ));
    }
    Some("[系统消息]".into())
}

pub(crate) fn parse_appmsg(text: &str) -> Option<String> {
    if let Some(parsed) = parse_appmsg_dom(text) {
        return Some(parsed);
    }
    parse_appmsg_legacy(text)
}

pub(crate) fn parse_appmsg_dom(text: &str) -> Option<String> {
    let doc = Document::parse(text).ok()?;
    let appmsg = doc.descendants().find(|node| node.has_tag_name("appmsg"))?;
    let title = xml_text(xml_child(appmsg, "title")).unwrap_or_default();
    let atype = xml_text(xml_child(appmsg, "type")).unwrap_or_default();
    match atype.as_str() {
        "2000" => Some(crate::message::transfer::summary(
            super::transfer::extract(appmsg).as_ref(),
            &title,
        )),
        "6" => Some(format_file_appmsg(appmsg, &title)),
        "19" => Some(format_record_appmsg(appmsg, &title)),
        _ => None,
    }
}

pub(crate) fn parse_appmsg_legacy(text: &str) -> Option<String> {
    let title = extract_xml_text(text, "title")?;
    let atype = extract_xml_text(text, "type").unwrap_or_default();
    match atype.as_str() {
        "6" => Some(if !title.is_empty() {
            format!("[文件] {}", title)
        } else {
            "[文件]".into()
        }),
        "57" => {
            let ref_content = quote_refermsg_content(text)
                .or_else(|| {
                    extract_xml_text(text, "content").and_then(|s| quote_content_text(&s, 40))
                })
                .unwrap_or_default();
            let quote = if !title.is_empty() {
                format!("[引用] {}", title)
            } else {
                "[引用]".into()
            };
            if !ref_content.is_empty() {
                Some(format!("{}\n  \u{21b3} {}", quote, ref_content))
            } else {
                Some(quote)
            }
        }
        "33" | "36" | "44" => Some(if !title.is_empty() {
            format!("[小程序] {}", title)
        } else {
            "[小程序]".into()
        }),
        _ => Some(if !title.is_empty() {
            format!("[链接] {}", title)
        } else {
            "[链接/文件]".into()
        }),
    }
}

pub(crate) fn format_file_appmsg<'a, 'input>(appmsg: Node<'a, 'input>, title: &str) -> String {
    let mut meta = Vec::new();
    if let Some(size) = xml_child(appmsg, "appattach")
        .and_then(|attach| xml_text(xml_child(attach, "totallen")))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|size| *size > 0)
    {
        meta.push(format_byte_size(size));
    }
    if let Some(ext) = xml_child(appmsg, "appattach")
        .and_then(|attach| xml_text(xml_child(attach, "fileext")))
        .filter(|ext| !ext.is_empty())
    {
        meta.push(ext);
    }

    let base = if !title.is_empty() {
        format!("[文件] {}", title)
    } else {
        "[文件]".into()
    };
    if meta.is_empty() {
        base
    } else {
        format!("{} ({})", base, meta.join(", "))
    }
}

pub(crate) fn format_record_appmsg<'a, 'input>(appmsg: Node<'a, 'input>, title: &str) -> String {
    let items = record_item_lines(appmsg);
    let mut header = if !title.is_empty() {
        format!("[合并聊天记录] {}", title)
    } else {
        "[合并聊天记录]".into()
    };
    if !items.is_empty() {
        header.push_str(&format!(" ({}条)", items.len()));
    }

    let mut lines = vec![header];
    if items.is_empty() {
        if let Some(desc) = xml_text(xml_child(appmsg, "des")).filter(|desc| !desc.is_empty()) {
            lines.push(format!("  {}", collapse_text(&desc, 120)));
        }
    } else {
        for item in items.iter().take(10) {
            lines.push(format!("  - {}", item));
        }
        if items.len() > 10 {
            lines.push(format!("  - ... 还有{}条", items.len() - 10));
        }
    }
    lines.join("\n")
}

pub(crate) fn record_item_lines<'a, 'input>(appmsg: Node<'a, 'input>) -> Vec<String> {
    let mut lines = record_item_lines_from_node(appmsg);
    if !lines.is_empty() {
        return lines;
    }

    let Some(record_xml) =
        xml_text(xml_child(appmsg, "recorditem")).filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    let unescaped = unescape_html(&record_xml);
    for candidate in [&record_xml, &unescaped] {
        if let Ok(doc) = Document::parse(candidate) {
            lines = record_item_lines_from_node(doc.root_element());
            if !lines.is_empty() {
                break;
            }
        }
    }
    lines
}

pub(crate) fn record_item_lines_from_node<'a, 'input>(node: Node<'a, 'input>) -> Vec<String> {
    node.descendants()
        .filter(|child| child.has_tag_name("dataitem"))
        .filter_map(format_record_item)
        .collect()
}

pub(crate) fn format_record_item<'a, 'input>(item: Node<'a, 'input>) -> Option<String> {
    let name = first_child_text(item, &["sourcename", "datasrcname", "sourceusername"]);
    let desc = first_child_text(item, &["datadesc", "datatitle", "datafmt"]).or_else(|| {
        item.attribute("datatype")
            .and_then(record_datatype_label)
            .map(str::to_string)
    })?;
    let desc = collapse_text(&desc, 100);
    if let Some(name) = name.filter(|value| !value.is_empty()) {
        Some(format!("{}: {}", name, desc))
    } else {
        Some(desc)
    }
}

pub(crate) fn first_child_text<'a, 'input>(
    node: Node<'a, 'input>,
    tags: &[&str],
) -> Option<String> {
    tags.iter()
        .find_map(|tag| xml_text(xml_child(node, tag)))
        .filter(|value| !value.is_empty())
}

pub(crate) fn record_datatype_label(datatype: &str) -> Option<&'static str> {
    match datatype {
        "1" => Some("[文本]"),
        "2" => Some("[图片]"),
        "3" => Some("[语音]"),
        "4" => Some("[视频]"),
        "6" => Some("[文件]"),
        "17" => Some("[链接]"),
        _ => None,
    }
}

pub(crate) fn quote_refermsg_content(text: &str) -> Option<String> {
    let refer = extract_xml_text(text, "refermsg")?;
    let content = extract_xml_text(&refer, "content")
        .and_then(|s| quote_content_text(&s, 80))
        .or_else(|| {
            extract_xml_text(&refer, "type")
                .and_then(|t| quote_refermsg_type_label(&t).map(str::to_string))
        })?;
    match extract_xml_text(&refer, "displayname") {
        Some(name) if !name.is_empty() => Some(format!("{}: {}", name, content)),
        _ => Some(content),
    }
}

pub(crate) fn quote_content_text(raw: &str, max_chars: usize) -> Option<String> {
    let unescaped = unescape_html(raw);
    if unescaped.contains("<appmsg") {
        if let Some(parsed) = parse_appmsg(&unescaped) {
            return Some(parsed);
        }
    }
    let collapsed = collapse_text(&unescaped, max_chars);
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

pub(crate) fn quote_refermsg_type_label(t: &str) -> Option<&'static str> {
    match t {
        "1" => None,
        "3" => Some("[图片]"),
        "34" => Some("[语音]"),
        "43" => Some("[视频]"),
        "47" => Some("[表情]"),
        "49" => Some("[链接/文件]"),
        _ => None,
    }
}

pub(crate) fn collapse_text(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max_chars {
        format!(
            "{}...",
            collapsed.chars().take(max_chars).collect::<String>()
        )
    } else {
        collapsed
    }
}

pub(crate) fn format_byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GB {
        format_decimal_unit(bytes_f / GB, "GB")
    } else if bytes_f >= MB {
        format_decimal_unit(bytes_f / MB, "MB")
    } else if bytes_f >= KB {
        format_decimal_unit(bytes_f / KB, "KB")
    } else {
        format!("{} B", bytes)
    }
}

pub(crate) fn format_decimal_unit(value: f64, unit: &str) -> String {
    let mut s = format!("{:.1}", value);
    if s.ends_with(".0") {
        s.truncate(s.len() - 2);
    }
    format!("{} {}", s, unit)
}

pub(crate) fn appmsg_url_for_message(local_type: i64, content: &str) -> Option<String> {
    if (local_type as u64 & 0xFFFFFFFF) != 49 {
        return None;
    }
    extract_appmsg_url(content)
}

pub(crate) fn extract_appmsg_url(text: &str) -> Option<String> {
    let xml = strip_group_prefix(text);
    if !xml.contains("<appmsg") {
        return None;
    }
    if extract_xml_text(&xml, "type").as_deref() == Some("57") {
        return None;
    }
    let url = extract_xml_text(&xml, "url")
        .or_else(|| extract_xml_text(&xml, "url1"))
        .map(|s| unescape_html(strip_xml_cdata(&s)))?;
    if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some(url)
}
