//! 对照 vendor/wechat-decrypt/chat_export_helpers.py 的正文与附加字段契约。
//! 输入为已解压、已剥离群发送者前缀的正文；不读取数据库、联系人或附件。
//! 引用名称可由调用方传入上下文；无上下文时保留 XML 显示名或原始账号。
//! XML 使用现有安全边界；整数子类型仅承诺 ASCII 十进制（含符号和下划线）。

use base64::engine::{general_purpose::GeneralPurpose, DecodePaddingMode, GeneralPurposeConfig};
use base64::Engine;
use roxmltree::Node;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// 联系人和自身账号由调用方加载；正文解析本身不进行外部查询。
#[derive(Debug, Default)]
pub struct ExportContext<'a> {
    pub is_group: bool,
    pub chat_username: &'a str,
    pub chat_display_name: &'a str,
    pub self_username: &'a str,
    pub names: Option<&'a HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
pub struct ExportContent {
    // None 表示省略正文，Some("") 表示必须保留空正文。
    pub content: Option<String>,
    pub extras: Map<String, Value>,
}

// 保留已交付的 Result 接口；当前已确认的旧 app 分支不再返回此错误。
#[derive(Debug, PartialEq, Eq)]
pub struct UnsupportedAppType(pub u32);

impl std::fmt::Display for UnsupportedAppType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "原生导出正文尚未覆盖 appmsg type={}", self.0)
    }
}

impl std::error::Error for UnsupportedAppType {}

/// 返回旧导出的正文及扩展字段；extras 为空等价于旧函数返回 None。
#[cfg(test)]
pub fn extract(
    local_type: i64,
    content: Option<&str>,
) -> Result<ExportContent, UnsupportedAppType> {
    extract_with_context(local_type, content, &ExportContext::default())
}

pub fn extract_with_context(
    local_type: i64,
    content: Option<&str>,
    context: &ExportContext<'_>,
) -> Result<ExportContent, UnsupportedAppType> {
    let mut out = ExportContent {
        content: None,
        extras: Map::new(),
    };
    let Some(content) = content else {
        return Ok(out);
    };
    let (base, subtype) = if local_type > u32::MAX as i64 {
        (local_type & 0xffff_ffff, local_type >> 32)
    } else {
        (local_type, 0)
    };
    out.content = match base {
        1 => Some(content.to_owned()),
        43 => Some(super::summary::video(content)),
        47 => Some(sticker(content)),
        49 => return app(content, subtype, context),
        50 => super::summary::voip(content),
        10000 => Some(system(content)),
        10002 => Some("[撤回消息]".into()),
        // 图片、语音、名片、位置及未识别顶层类型：旧导出不输出阅读摘要。
        _ => None,
    };
    Ok(out)
}

fn descendant<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.descendants()
        .skip(1)
        .find(|n| n.has_tag_name(tag) && n.tag_name().namespace().is_none())
}

fn child_text<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> &'a str {
    node.children()
        .find(|n| n.has_tag_name(tag) && n.tag_name().namespace().is_none())
        .and_then(|n| n.text())
        .unwrap_or("")
}

fn system(content: &str) -> String {
    if content.is_empty() {
        return "[系统消息]".into();
    }
    if content.contains("<sysmsg") {
        if let Some(doc) = super::xml::parse(content) {
            if let Some(text) = descendant(doc.root_element(), "content").and_then(|n| n.text()) {
                if !text.is_empty() {
                    return text.trim().to_owned();
                }
            }
        }
    }
    content.to_owned()
}

fn sticker(content: &str) -> String {
    let label = super::xml::parse(content).and_then(|doc| {
        let emoji = descendant(doc.root_element(), "emoji")?;
        decode_sticker_desc(emoji.attribute("desc")?)
    });
    label
        .map(|text| format!("[表情] {text}"))
        .unwrap_or_else(|| "[表情]".into())
}

fn decode_sticker_desc(desc: &str) -> Option<String> {
    if !desc.is_ascii() {
        return None;
    }
    // Python b64decode 默认忽略非字母表字符，且遇到完整填充后忽略尾部。
    let mut encoded = Vec::new();
    let mut padding = 0;
    let mut terminated = false;
    for byte in desc.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') {
            padding = 0;
            encoded.push(byte);
        } else if byte == b'=' {
            match encoded.len() % 4 {
                2 => {
                    padding += 1;
                    if padding == 2 {
                        terminated = true;
                        break;
                    }
                }
                3 => {
                    terminated = true;
                    break;
                }
                _ => {}
            }
        }
    }
    // 旧实现不接受缺失填充；不要顺便修复损坏的描述。
    let remainder = encoded.len() % 4;
    if remainder == 1 || (remainder != 0 && !terminated) {
        return None;
    }
    let engine = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let raw = engine.decode(encoded).ok()?;
    let index = raw.windows(7).position(|part| part == b"default")?;
    if *raw.get(index + 7)? != 0x12 {
        return None;
    }
    // 有意保留旧代码的单字节长度及切片截断行为，不升级为 protobuf varint。
    let len = *raw.get(index + 8)? as usize;
    let start = index + 9;
    let bytes = raw.get(start..raw.len().min(start + len))?;
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.is_empty()).then(|| text.to_owned())
}

fn integer(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (negative, digits) = if let Some(rest) = raw.strip_prefix('-') {
        (true, rest)
    } else {
        (false, raw.strip_prefix('+').unwrap_or(raw))
    };
    if digits
        .split('_')
        .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let digits = digits.replace('_', "");
    let digits = digits.trim_start_matches('0');
    Some(if digits.is_empty() {
        "0".into()
    } else {
        format!("{}{digits}", if negative { "-" } else { "" })
    })
}

fn app(
    content: &str,
    subtype: i64,
    context: &ExportContext<'_>,
) -> Result<ExportContent, UnsupportedAppType> {
    let mut out = ExportContent {
        content: None,
        extras: Map::new(),
    };
    if !content.contains("<appmsg") {
        return Ok(out);
    }
    // 仅旧代码的精确 type=19 标记允许放宽外层 XML，不能扩大其他类型的解析上限。
    let doc = super::xml::parse(content).or_else(|| {
        let upper = content.to_ascii_uppercase();
        (content.contains("<type>19</type>")
            && content.chars().take(500_001).count() <= 500_000
            && !upper.contains("<!DOCTYPE")
            && !upper.contains("<!ENTITY"))
        .then(|| roxmltree::Document::parse(content).ok())
        .flatten()
    });
    let Some(doc) = doc else { return Ok(out) };
    let Some(node) = descendant(doc.root_element(), "appmsg") else {
        return Ok(out);
    };
    let title = super::xml::collapse(child_text(node, "title"));
    let kind = integer(child_text(node, "type")).unwrap_or_else(|| subtype.to_string());
    let label = match kind.as_str() {
        "19" | "57" | "51" | "2001" => {
            out.content = Some(match kind.as_str() {
                "19" => record(node, &title),
                "57" => refer(node, &title, context),
                "51" => finder(node, &title),
                _ => redpacket(node, &title),
            });
            return Ok(out);
        }
        "2000" => {
            out.content = Some(super::transfer::summary(node, &title));
            // extras 检测不使用高位 subtype 回退，这是旧实现与正文分支的差异。
            if integer(&super::xml::collapse(child_text(node, "type"))).as_deref() == Some("2000") {
                // 现有转账入口采用 Rust 整数语法；仅规范化已确认的 type，保留其余字段。
                let type_node = node
                    .children()
                    .find(|n| n.has_tag_name("type") && n.tag_name().namespace().is_none())
                    .unwrap();
                let mut normalized = content.to_owned();
                normalized.replace_range(type_node.range(), "<type>2000</type>");
                if let Some(fields) = super::transfer::parse(&normalized)
                    .ok()
                    .and_then(|value| value.export_fields())
                {
                    out.extras.insert("type".into(), "transfer".into());
                    out.extras.insert("transfer".into(), Value::Object(fields));
                }
            }
            return Ok(out);
        }
        "6" => "[文件]",
        "5" => "[链接]",
        "33" | "36" | "44" => "[小程序]",
        _ => "[链接/文件]",
    };
    out.content = Some(if title.is_empty() {
        label.into()
    } else {
        format!("{label} {title}")
    });
    Ok(out)
}

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.has_tag_name(tag) && n.tag_name().namespace().is_none())
}

fn path_text<'a, 'input>(node: Node<'a, 'input>, path: &str) -> &'a str {
    path.split('/')
        .try_fold(node, child)
        .and_then(|n| n.text())
        .unwrap_or("")
}

fn titled(label: &str, title: &str) -> String {
    if title.is_empty() {
        format!("[{label}]")
    } else {
        format!("[{label}] {title}")
    }
}

fn finder(node: Node<'_, '_>, title: &str) -> String {
    let nickname = super::xml::collapse(path_text(node, "finderFeed/nickname"));
    let desc = super::xml::collapse(path_text(node, "finderFeed/desc"));
    if nickname.is_empty() {
        return titled("视频号", title);
    }
    if desc.is_empty() {
        return titled("视频号", &nickname);
    }
    format!(
        "[视频号] {nickname}: {}",
        desc.chars().take(80).collect::<String>()
    )
}

fn redpacket(node: Node<'_, '_>, title: &str) -> String {
    let Some(info) = child(node, "wcpayinfo") else {
        return titled("红包", title);
    };
    let scene = super::xml::collapse(child_text(info, "scenetext"));
    let greeting = super::xml::collapse(child_text(info, "sendertitle"));
    let mut parts = vec![if scene.is_empty() {
        "[红包]".into()
    } else {
        format!("[红包·{scene}]")
    }];
    if !greeting.is_empty() {
        parts.push(greeting)
    }
    // 仅这两个场景允许从 senderdes 提取金额，不推测普通红包金额。
    if matches!(scene.as_str(), "群收款" | "活动账单") {
        static AMOUNT: OnceLock<regex::Regex> = OnceLock::new();
        let re = AMOUNT.get_or_init(|| regex::Regex::new(r"(\d+(?:\.\d+)?)\s*元").unwrap());
        if let Some(capture) = re.captures(child_text(info, "senderdes")) {
            parts.push(format!("人均 {} 元", &capture[1]));
        }
    }
    static SENDER: OnceLock<regex::Regex> = OnceLock::new();
    let re = SENDER.get_or_init(|| regex::Regex::new(r"sendusername=([^&]+)").unwrap());
    if let Some(capture) = re.captures(child_text(info, "nativeurl")) {
        parts.push(format!("(发自 {})", &capture[1]));
    }
    parts.join(" ")
}

fn quote_sender(user: &str, display: &str, context: &ExportContext<'_>) -> String {
    let empty = HashMap::new();
    let names = context.names.unwrap_or(&empty);
    if !user.is_empty() {
        if !context.is_group
            && user != context.chat_username
            && user != context.self_username
            && !names.contains_key(user)
            && !display.is_empty()
        {
            return display.to_owned();
        }
        // 群聊账号恰等于聊天账号时仍按引用账号解析，prefix 因此也传入 user。
        return super::identity::export_sender(
            user,
            user,
            context.is_group,
            context.chat_username,
            context.chat_display_name,
            context.self_username,
            names,
        );
    }
    if !context.is_group
        && !display.is_empty()
        && display != context.chat_display_name
        && !context.self_username.is_empty()
    {
        let me = names
            .get(context.self_username)
            .map(String::as_str)
            .unwrap_or(context.self_username);
        if !me.is_empty() && display == me {
            return "me".into();
        }
    }
    display.to_owned()
}

pub(crate) fn refer_label(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "1" => "文本",
        "3" => "图片",
        "34" => "语音",
        "42" => "名片",
        "43" => "视频",
        "47" => "动画表情",
        "48" => "位置",
        "49" => "链接/卡片",
        "50" => "通话",
        _ => return None,
    })
}

pub(crate) fn refer_summary(kind: &str, content: &str) -> String {
    if content.is_empty() {
        return match refer_label(kind) {
            Some(label) => titled(label, ""),
            None if kind.is_empty() => "[引用消息]".into(),
            None => format!("[type={kind}]"),
        };
    }
    if kind == "1" {
        let text = super::xml::collapse(content);
        return truncate(&text, 160);
    }
    if kind == "49" {
        let Some(doc) = super::xml::parse(content) else {
            return "[卡片]".into();
        };
        let Some(inner) = descendant(doc.root_element(), "appmsg") else {
            return "[卡片]".into();
        };
        let inner_type = super::xml::collapse(child_text(inner, "type"));
        let title = super::xml::collapse(child_text(inner, "title"));
        let label = match inner_type.as_str() {
            "5" => "链接",
            "6" => "文件",
            "8" => "动画表情卡",
            "19" => "聊天记录",
            "33" | "36" => "小程序",
            "51" => "视频号",
            "57" => "引用消息",
            "2000" => "转账",
            "2001" => "红包",
            "" => "卡片",
            _ => return titled(&format!("卡片 type={inner_type}"), &title),
        };
        return titled(label, &title);
    }
    refer_label(kind)
        .map(|label| titled(label, ""))
        .unwrap_or_else(|| format!("[type={kind}]"))
}

fn refer(node: Node<'_, '_>, title: &str, context: &ExportContext<'_>) -> String {
    let reply = if title.is_empty() {
        "[引用消息]"
    } else {
        title
    };
    let Some(info) = child(node, "refermsg") else {
        return reply.to_owned();
    };
    let kind = super::xml::collapse(child_text(info, "type"));
    let summary = refer_summary(&kind, child_text(info, "content"));
    let user = super::xml::collapse(child_text(info, "fromusr"));
    let display = super::xml::collapse(child_text(info, "displayname"));
    let sender = quote_sender(&user, &display, context);
    let prefix = if sender.is_empty() {
        "回复: ".into()
    } else {
        format!("回复 {sender}: ")
    };
    format!("{reply}\n  ↳ {prefix}{summary}")
}

fn truncate(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push('…')
    }
    out
}

fn record_item(item: Node<'_, '_>) -> String {
    let kind = item.attribute("datatype").unwrap_or("").trim();
    let title = || super::xml::collapse(child_text(item, "datatitle"));
    let desc = || super::xml::collapse(child_text(item, "datadesc"));
    match kind {
        "1" => {
            let text = desc();
            if text.is_empty() {
                "[文本]".into()
            } else {
                text
            }
        }
        "2" => "[图片]".into(),
        "3" => "[名片]".into(),
        "4" => "[语音]".into(),
        "5" => "[视频]".into(),
        "7" => "[位置]".into(),
        "23" => "[视频号直播]".into(),
        "37" => "[表情包]".into(),
        "6" => titled("链接", &title()),
        "36" => titled("小程序/H5", &title()),
        "8" => titled("文件", &title()),
        "17" => titled("聊天记录", &title()),
        "19" => {
            let mut text = title();
            if text.is_empty() {
                text = super::xml::collapse(path_text(item, "appbranditem/sourcedisplayname"))
            }
            if text.is_empty() {
                text = "小程序".into()
            }
            titled("小程序", &text)
        }
        "22" => {
            let text = super::xml::collapse(path_text(item, "finderFeed/desc"));
            titled("视频号", &text.chars().take(80).collect::<String>())
        }
        "29" => {
            let song = title();
            let artist = desc();
            if !song.is_empty() && !artist.is_empty() {
                format!("[音乐] {song} - {artist}")
            } else {
                titled("音乐", &song)
            }
        }
        _ => {
            let text = desc();
            if !text.is_empty() {
                return text;
            }
            let text = title();
            if !text.is_empty() {
                text
            } else {
                format!("[未知类型 {kind}]")
            }
        }
    }
}

fn record(node: Node<'_, '_>, title: &str) -> String {
    let fallback = if title.is_empty() {
        "聊天记录"
    } else {
        title
    };
    let xml = child_text(node, "recorditem");
    if xml.is_empty() {
        return format!("[聊天记录] {fallback}（待加载）");
    }
    // 内层与旧代码一致只解析一层；嵌套记录作为标签，不递归读取附件。
    let upper = xml.to_ascii_uppercase();
    let doc = if xml.chars().take(500_001).count() > 500_000
        || upper.contains("<!DOCTYPE")
        || upper.contains("<!ENTITY")
    {
        None
    } else {
        roxmltree::Document::parse(xml).ok()
    };
    let Some(doc) = doc else {
        return format!("[聊天记录] {fallback}");
    };
    let root = doc.root_element();
    let inner_title = super::xml::collapse(child_text(root, "title"));
    let title = if inner_title.is_empty() {
        fallback
    } else {
        &inner_title
    };
    let group = child_text(root, "isChatRoom").trim() == "1";
    let items: Vec<_> = child(root, "datalist")
        .map(|list| {
            list.children()
                .filter(|n| n.has_tag_name("dataitem") && n.tag_name().namespace().is_none())
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        let suffix = if group {
            "（群聊转发，待加载）"
        } else {
            "（待加载）"
        };
        return format!("[聊天记录] {title}{suffix}");
    }
    let group_label = if group { "（群聊转发）" } else { "" };
    let mut lines = vec![format!(
        "[聊天记录] {title}{group_label}，共 {} 条:",
        items.len()
    )];
    for (index, item) in items.iter().take(50).enumerate() {
        let sender = super::xml::collapse(child_text(*item, "sourcename"));
        let when = super::xml::collapse(child_text(*item, "sourcetime"));
        let mut prefix = format!("[{index}]");
        for text in [&when, &sender] {
            if !text.is_empty() {
                prefix.push(' ');
                prefix.push_str(text)
            }
        }
        lines.push(format!(
            "  {prefix}: {}",
            truncate(&record_item(*item), 200)
        ));
    }
    if items.len() > 50 {
        lines.push(format!("  …（还有 {} 条未显示）", items.len() - 50))
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_ast_extracted_legacy_golden() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/export-content-golden.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let ctx = &case["context"];
            let names: std::collections::HashMap<String, String> =
                serde_json::from_value(ctx["names"].clone()).unwrap_or_default();
            let context = super::ExportContext {
                is_group: ctx["is_group"].as_bool().unwrap_or(false),
                chat_username: ctx["chat_username"].as_str().unwrap_or(""),
                chat_display_name: ctx["chat_display_name"].as_str().unwrap_or(""),
                self_username: ctx["self_username"].as_str().unwrap_or(""),
                names: Some(&names),
            };
            let actual = super::extract_with_context(
                case["type"].as_i64().unwrap(),
                case["input"].as_str(),
                &context,
            );
            assert_eq!(
                serde_json::to_value(actual.unwrap()).unwrap(),
                case["expected"],
                "{}",
                case["name"]
            );
            if ctx.is_null() {
                assert_eq!(
                    serde_json::to_value(
                        super::extract(case["type"].as_i64().unwrap(), case["input"].as_str())
                            .unwrap()
                    )
                    .unwrap(),
                    case["expected"],
                    "{}",
                    case["name"]
                );
            }
        }
    }
}
