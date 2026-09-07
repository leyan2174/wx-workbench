//! 对齐旧版内容解码启发式；不是企业微信消息协议的完整反序列化器。
use anyhow::Result;
use chrono::{Local, TimeZone};
use regex::Regex;
use rusqlite::types::ValueRef;
use std::sync::OnceLock;

fn printable(c: char) -> bool {
    static NONPRINT: OnceLock<Regex> = OnceLock::new();
    let mut buf = [0u8; 4];
    c == ' '
        || !NONPRINT
            .get_or_init(|| Regex::new(r"[\p{C}\p{Z}]").unwrap())
            .is_match(c.encode_utf8(&mut buf))
}

fn clean(text: &str) -> String {
    let mut result = String::new();
    let mut spaces = false;
    let mut newlines = 0;
    for c in text.chars() {
        let c = if c == '\n' || c == '\t' || printable(c) {
            c
        } else {
            ' '
        };
        if c == ' ' || c == '\t' {
            if !spaces {
                result.push(' ');
            }
            spaces = true;
            newlines = 0;
        } else if c == '\n' {
            if newlines < 2 {
                result.push(c);
            }
            newlines += 1;
            spaces = false;
        } else {
            result.push(c);
            spaces = false;
            newlines = 0;
        }
    }
    result.trim().into()
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos)?;
        *pos += 1;
        if shift == 63 && b > 1 {
            return None;
        }
        value |= u64::from(b & 127) << shift;
        if b & 128 == 0 {
            return Some(value);
        }
    }
    None
}

fn segment_text(data: &[u8]) -> Option<String> {
    if data.contains(&0) {
        return None;
    }
    let text = clean(std::str::from_utf8(data).ok()?);
    if text.chars().count() < 2 || (text.len() >= 32 && text.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        None
    } else {
        Some(text)
    }
}

fn protobuf_strings(data: &[u8], depth: usize) -> Option<Vec<String>> {
    if depth > 4 || data.is_empty() {
        return None;
    }
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < data.len() {
        let tag = varint(data, &mut pos)?;
        if tag == 0 {
            return None;
        }
        match tag & 7 {
            0 => {
                varint(data, &mut pos)?;
            }
            1 => pos = pos.checked_add(8)?,
            5 => pos = pos.checked_add(4)?,
            2 => {
                let len = usize::try_from(varint(data, &mut pos)?).ok()?;
                let end = pos.checked_add(len)?;
                let segment = data.get(pos..end)?;
                pos = end;
                if let Some(text) = segment_text(segment) {
                    out.push(text);
                } else if let Some(nested) = protobuf_strings(segment, depth + 1) {
                    out.extend(nested);
                }
            }
            _ => return None,
        }
        if pos > data.len() {
            return None;
        }
    }
    Some(out)
}

fn decode_gbk(data: &[u8]) -> String {
    // Python GBK 拒绝独立 0x80；encoding_rs 的网页兼容映射会将其变成欧元符号。
    let mut output = String::new();
    let mut pos = 0;
    while pos < data.len() {
        if data[pos] == 0x80 {
            output.push('\u{fffd}');
            pos += 1;
            continue;
        }
        let length = if (0x81..=0xfe).contains(&data[pos])
            && data
                .get(pos + 1)
                .is_some_and(|b| (0x40..=0xfe).contains(b) && *b != 0x7f)
        {
            2
        } else {
            1
        };
        let (text, _) = encoding_rs::GBK.decode_without_bom_handling(&data[pos..pos + length]);
        output.push_str(&text);
        pos += length;
    }
    output
}

pub(super) fn decode_bytes(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let control = data
            .iter()
            .filter(|&&b| b < 32 && !matches!(b, 9 | 10 | 13))
            .count();
        let count = text.chars().count();
        let visible = text
            .chars()
            .filter(|&c| printable(c) || c == '\n' || c == '\t')
            .count();
        if control as f64 / data.len() as f64 <= 0.08 && visible as f64 / count.max(1) as f64 > 0.9
        {
            return clean(text);
        }
    }
    if let Some(values) = protobuf_strings(data, 0) {
        let mut texts = Vec::new();
        for value in values {
            if !value.is_empty() && !texts.contains(&value) {
                texts.push(value);
            }
        }
        if !texts.is_empty() {
            return texts.into_iter().take(12).collect::<Vec<_>>().join("\n");
        }
    }
    let utf8 = String::from_utf8_lossy(data);
    let gbk = decode_gbk(data);
    let (utf16, _) = encoding_rs::UTF_16LE.decode_without_bom_handling(data);
    for candidate in [utf8.as_ref(), gbk.as_str(), utf16.as_ref()] {
        let text = clean(&candidate);
        if !text.is_empty() && !text.chars().take(20).any(|c| c == '\u{fffd}') {
            return text.chars().take(2000).collect();
        }
    }
    format!("[二进制内容 {} 字节]", data.len())
}

pub(super) fn decode_value(value: ValueRef<'_>) -> Result<String> {
    Ok(match value {
        ValueRef::Null => String::new(),
        ValueRef::Text(text) => clean(std::str::from_utf8(text)?),
        ValueRef::Blob(data) => decode_bytes(data),
        _ => anyhow::bail!("Message content must be text, blob or NULL"),
    })
}

pub(super) fn type_name(value: i64) -> String {
    match value {
        0 => "文本/混合",
        2 => "文本",
        4 => "图片",
        7 => "语音",
        15 => "图片/文件",
        38 => "应用消息",
        40 => "通话/音视频",
        503 => "状态",
        1011 => "会议通知",
        _ => return format!("未知({value})"),
    }
    .into()
}

pub(super) fn format_time(value: i64) -> String {
    if value <= 0 {
        return String::new();
    }
    let seconds = if value > 20_000_000_000 {
        value / 1000
    } else {
        value
    };
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}
