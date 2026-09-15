//! Shared legacy WeChat label scalar and field-30 decoding rules.
pub(crate) fn parse_label_id(raw: &str) -> Option<i64> {
    // 对齐 oracle 使用的 Python Unicode 十进制数字表。
    const ZEROS: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
        0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60,
        0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0,
        0x1e950, 0x1fbf0,
    ];
    let normalized: String = raw
        .trim()
        .chars()
        .map(|c| {
            ZEROS
                .iter()
                .find(|&&zero| (zero..zero + 10).contains(&(c as u32)))
                .map(|&zero| (b'0' + (c as u32 - zero) as u8) as char)
                .unwrap_or(c)
        })
        .collect();
    let raw = normalized.as_str();
    let digits = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    if digits.is_empty() {
        return None;
    }
    // Python int 接受数字之间的下划线，但不接受首尾或连续下划线。
    if digits
        .split('_')
        .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    raw.replace('_', "").parse().ok()
}

pub(crate) fn sqlite_id_equal(a: &rusqlite::types::Value, b: &rusqlite::types::Value) -> bool {
    use rusqlite::types::Value::{Integer, Real};
    match (a, b) {
        (Integer(i), Real(f)) | (Real(f), Integer(i)) => {
            f.is_finite()
                && f.fract() == 0.0
                && *f >= i64::MIN as f64
                && *f < 9223372036854775808.0
                && *i == *f as i64
        }
        _ => a == b,
    }
}

fn varint(data: &[u8], pos: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    let mut shift = 0;
    while *pos < data.len() {
        let byte = data[*pos];
        *pos += 1;
        let bits = (byte & 127) as usize;
        if bits > (usize::MAX >> shift) {
            return None;
        }
        value |= bits << shift;
        if byte & 128 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift >= usize::BITS {
            return None;
        }
    }
    Some(value)
}

pub(crate) fn extract_field_30(data: &[u8]) -> Option<&str> {
    let mut pos = 0;
    while pos < data.len() {
        let tag = varint(data, &mut pos)?;
        match tag & 7 {
            0 => {
                while pos < data.len() && data[pos] & 128 != 0 {
                    pos += 1;
                }
                pos = pos.saturating_add(1);
            }
            1 => pos = pos.saturating_add(8),
            5 => pos = pos.saturating_add(4),
            2 => {
                let len = varint(data, &mut pos)?;
                let end = pos.saturating_add(len);
                // Python 切片允许声明长度超过末尾；不能用严格 protobuf 解码替换。
                if tag >> 3 == 30 {
                    return std::str::from_utf8(&data[pos..end.min(data.len())]).ok();
                }
                pos = end;
            }
            _ => break,
        }
    }
    None
}
