use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use regex::{Captures, Regex};
use std::{collections::HashMap, io::Read, sync::OnceLock};

pub const MAX_XML_CHARS: usize = 200_000;
const MAX_BYTES: usize = MAX_XML_CHARS * 4;

#[derive(Clone, Copy, Debug)]
pub enum Content<'a> {
    Null,
    Text(&'a str),
    Blob(&'a [u8]),
}

// Python 的 errors="ignore" 会丢弃非法 UTF-8，而不是插入替换字符。
fn utf8_ignore(mut bytes: &[u8]) -> String {
    let mut out = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(error) => {
                out.push_str(std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap());
                match error.error_len() {
                    Some(len) => bytes = &bytes[error.valid_up_to() + len..],
                    None => break,
                }
            }
        }
    }
    out
}

fn numeric_entity(value: &str) -> String {
    let value = value.trim_end_matches(';');
    let number = if value.starts_with("#x") || value.starts_with("#X") {
        u32::from_str_radix(&value[2..], 16)
    } else {
        value[1..].parse()
    };
    let Ok(mut n) = number else {
        return "\u{fffd}".into();
    };
    // HTML5 保留 Windows-1252 的历史映射。
    const C1: [u32; 32] = [
        0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
        0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
        0x2dc, 0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
    ];
    if (0x80..=0x9f).contains(&n) {
        n = C1[(n - 0x80) as usize];
    }
    if n == 0 || (0xd800..=0xdfff).contains(&n) || n > 0x10ffff {
        return "\u{fffd}".into();
    }
    if (1..=8).contains(&n)
        || n == 11
        || (14..=31).contains(&n)
        || n == 127
        || (0xfdd0..=0xfdef).contains(&n)
        || n & 0xffff == 0xfffe
        || n & 0xffff == 0xffff
    {
        return String::new();
    }
    char::from_u32(n).unwrap().to_string()
}

pub(super) fn html_unescape(text: &str) -> String {
    static ENTITIES: OnceLock<HashMap<String, String>> = OnceLock::new();
    static RE: OnceLock<Regex> = OnceLock::new();
    let entities =
        ENTITIES.get_or_init(|| serde_json::from_str(include_str!("html_entities.json")).unwrap());
    let re = RE.get_or_init(|| {
        Regex::new(r"&(#(?:[0-9]+|[xX][0-9a-fA-F]+);?|[^\t\n\x0c <&#;]{1,32};?)").unwrap()
    });
    re.replace_all(text, |c: &Captures<'_>| {
        let name = &c[1];
        if name.starts_with('#') {
            return numeric_entity(name);
        }
        if let Some(value) = entities.get(name) {
            return value.clone();
        }
        for (end, _) in name.char_indices().rev() {
            if let Some(value) = entities.get(&name[..end]) {
                return format!("{}{}", value, &name[end..]);
            }
        }
        c[0].to_owned()
    })
    .into_owned()
}

/// 支持 NULL、UTF-8/zstd BLOB、XML/hex/base64 TEXT；在解压阶段限制资源消耗。
pub fn decode_content(value: Content<'_>) -> Result<String> {
    let text = match value {
        Content::Null => String::new(),
        Content::Blob(raw) => {
            if raw.len() > MAX_BYTES * 2 {
                bail!("SNS encoded content exceeds limit");
            }
            let decoded;
            let bytes = if raw.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
                let mut decoder = zstd::stream::read::Decoder::new(raw)?;
                decoder.window_log_max(23)?;
                let mut output = Vec::new();
                decoder
                    .take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut output)?;
                if output.len() > MAX_BYTES {
                    bail!("SNS decompressed content exceeds limit");
                }
                decoded = output;
                decoded.as_slice()
            } else {
                raw
            };
            html_unescape(utf8_ignore(bytes).trim())
        }
        Content::Text(value) => {
            if value.len() > MAX_BYTES * 2 {
                bail!("SNS encoded content exceeds limit");
            }
            let value = value.trim();
            if value.starts_with('<') {
                html_unescape(value)
            } else {
                let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.len() >= 16
                    && compact.len().is_multiple_of(2)
                    && compact.bytes().all(|c| c.is_ascii_hexdigit())
                {
                    let bytes: Vec<u8> = compact
                        .as_bytes()
                        .chunks_exact(2)
                        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                        .collect();
                    return decode_content(Content::Blob(&bytes));
                }
                if compact.len() >= 24 && compact.len().is_multiple_of(4) {
                    if let Ok(bytes) = STANDARD.decode(&compact) {
                        return decode_content(Content::Blob(&bytes));
                    }
                }
                html_unescape(value)
            }
        }
    };
    if text.chars().count() > MAX_XML_CHARS {
        bail!("SNS XML exceeds character limit");
    }
    let upper = text.to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        bail!("SNS XML entity declarations forbidden");
    }
    Ok(text)
}

fn escape_ampersands(text: &str) -> String {
    static ENTITY: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY
        .get_or_init(|| Regex::new(r"^(?:amp|lt|gt|quot|apos|#[0-9]+|#x[0-9a-fA-F]+);").unwrap());
    let mut out = String::new();
    for (i, ch) in text.char_indices() {
        if ch == '&' && !re.is_match(&text[i + 1..]) {
            out.push_str("&amp;");
        } else {
            out.push(ch);
        }
    }
    out
}

/// 忠实保留旧清洗顺序，包括文本节点中 CDATA 标记被当成普通文本的行为。
pub fn sanitize_xml(text: &str) -> String {
    static CDATA: OnceLock<Regex> = OnceLock::new();
    static OPEN: OnceLock<Regex> = OnceLock::new();
    let clean: String = text
        .chars()
        .filter(|&c| !matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}'))
        .collect();
    let mut out = String::new();
    let mut last = 0;
    for m in CDATA
        .get_or_init(|| Regex::new(r"(?s)<!\[CDATA\[.*?\]\]>").unwrap())
        .find_iter(&clean)
    {
        out.push_str(&escape_ampersands(&clean[last..m.start()]));
        out.push_str(m.as_str());
        last = m.end();
    }
    out.push_str(&escape_ampersands(&clean[last..]));
    let open = OPEN.get_or_init(|| Regex::new(r"<(content|title|description|nickname|contentDesc|appname|sourceName|sourcename|poiName|displayName|feeddesc)\b[^>]*>").unwrap());
    let mut result = String::new();
    let mut cursor = 0;
    while let Some(c) = open.captures(&out[cursor..]) {
        let m = c.get(0).unwrap();
        let start = cursor + m.start();
        let end = cursor + m.end();
        let closing = format!("</{}>", &c[1]);
        result.push_str(&out[cursor..end]);
        if let Some(offset) = out[end..].find(&closing) {
            let close = end + offset;
            result.push_str(&out[end..close].replace('<', "&lt;").replace('>', "&gt;"));
            result.push_str(&closing);
            cursor = close + closing.len();
        } else {
            cursor = end;
        }
        debug_assert!(cursor > start);
    }
    result.push_str(&out[cursor..]);
    result
}
