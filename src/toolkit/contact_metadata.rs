//! 旧 export_all_chats 联系人元数据查询；不发现账号、不写数据库或导出文件。
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Default)]
pub struct ContactMetadata {
    pub fields: Map<String, Value>,
    /// 兼容回退的原因；不应合并进旧导出字段。
    pub diagnostics: Vec<String>,
}

/// 群聊直接省略元数据，甚至不打开路径。其他情况只读打开指定数据库。
pub fn contact_metadata_for_export(
    contact_db: &Path,
    username: &str,
    is_group: bool,
) -> ContactMetadata {
    if is_group {
        return ContactMetadata::default();
    }
    let mut result = ContactMetadata::default();
    for key in ["contact_remark", "contact_nick_name", "contact_memo"] {
        result
            .fields
            .insert(key.into(), Value::String(String::new()));
    }
    result
        .fields
        .insert("contact_tags".into(), Value::Array(vec![]));
    let conn = match Connection::open_with_flags(contact_db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(error) => {
            result
                .diagnostics
                .push(format!("contact database open (read-only): {error}"));
            return result;
        }
    };
    let snapshot = match conn.unchecked_transaction() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            result
                .diagnostics
                .push(format!("contact read snapshot: {error}"));
            return result;
        }
    };
    match contact_fields(&snapshot, username) {
        Ok((fields, diagnostics)) => {
            result.fields.extend(fields);
            result.diagnostics.extend(diagnostics);
        }
        Err(error) => result
            .diagnostics
            .push(format!("contact fields fallback: {error}")),
    }
    match contact_tags(&snapshot, username) {
        Ok(tags) => {
            result
                .fields
                .insert("contact_tags".into(), Value::Array(tags));
        }
        Err(error) => result
            .diagnostics
            .push(format!("contact tags fallback: {error}")),
    }
    result
}

fn contact_fields(
    conn: &Connection,
    username: &str,
) -> rusqlite::Result<(Map<String, Value>, Vec<String>)> {
    let mut stmt = conn.prepare("PRAGMA table_info(contact)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // 必需列交由 SQLite 解析（大小写不敏感）；可选列沿用 Python 精确匹配。
    let has_description = columns.iter().any(|c| c == "description");
    let has_local_type = columns.iter().any(|c| c == "local_type");
    let mut diagnostics = Vec::new();
    if !has_description {
        diagnostics.push("contact.description missing: empty memo".into());
    }
    if !has_local_type {
        diagnostics.push("contact.local_type missing: legacy unfiltered schema".into());
    }
    let description = if has_description {
        "[description]"
    } else {
        "NULL"
    };
    let filter = if has_local_type {
        " WHERE local_type != 3"
    } else {
        ""
    };
    // 旧 helper 先读取全表再取首个匹配项，保持重复 username 的首行语义。
    let mut stmt = conn.prepare(&format!(
        "SELECT username, nick_name, remark, {description} FROM contact{filter}"
    ))?;
    let mut rows = stmt.query([])?;
    let mut found = None;
    while let Some(row) = rows.next()? {
        if matches!(row.get_ref(0)?, ValueRef::Text(bytes) if bytes == username.as_bytes())
            && found.is_none()
        {
            let mut fields = Map::new();
            for (index, key) in [
                (1, "contact_nick_name"),
                (2, "contact_remark"),
                (3, "contact_memo"),
            ] {
                fields.insert(key.into(), python_or_empty(row.get_ref(index)?)?);
            }
            found = Some(fields);
        }
    }
    Ok((found.unwrap_or_default(), diagnostics))
}

fn python_or_empty(value: ValueRef<'_>) -> rusqlite::Result<Value> {
    Ok(match value {
        ValueRef::Null | ValueRef::Integer(0) => Value::String(String::new()),
        ValueRef::Real(0.0) => Value::String(String::new()),
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(n) => Value::from(n),
        ValueRef::Text(bytes) => Value::String(
            std::str::from_utf8(bytes)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?
                .into(),
        ),
        ValueRef::Blob([]) => Value::String(String::new()),
        ValueRef::Blob(_) => {
            return Err(rusqlite::Error::InvalidColumnType(
                0,
                "non-JSON blob metadata".into(),
                rusqlite::types::Type::Blob,
            ))
        }
    })
}

fn contact_tags(conn: &Connection, username: &str) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT label_id_, label_name_, sort_order_ FROM contact_label ORDER BY sort_order_",
    )?;
    // Python dict 覆盖重复 ID 的值，但不改变其首次插入顺序。
    let mut labels: Vec<(rusqlite::types::Value, Value, usize)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id = row.get::<_, rusqlite::types::Value>(0)?;
        let name = python_or_empty(row.get_ref(1)?)?;
        if let Some(label) = labels
            .iter_mut()
            .find(|(old, _, _)| sqlite_id_equal(old, &id))
        {
            label.1 = name;
        } else {
            labels.push((id, name, 0));
        }
    }
    if labels.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt =
        conn.prepare("SELECT username, extra_buffer FROM contact WHERE extra_buffer IS NOT NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let raw = row.get_ref(1)?;
        let buffer = match raw {
            ValueRef::Blob(bytes) => bytes,
            ValueRef::Text([]) => continue,
            ValueRef::Integer(0) | ValueRef::Real(0.0) => continue,
            _ => {
                return Err(rusqlite::Error::InvalidColumnType(
                    1,
                    "extra_buffer".into(),
                    raw.data_type(),
                ))
            }
        };
        let Some(ids) = extract_field_30(buffer) else {
            continue;
        };
        let is_target = !username.is_empty()
            && matches!(row.get_ref(0)?, ValueRef::Text(bytes) if bytes == username.as_bytes());
        if !is_target {
            continue;
        }
        for id in ids.split(',').filter_map(parse_label_id) {
            if let Some(label) = labels
                .iter_mut()
                .find(|(old, _, _)| sqlite_id_equal(old, &rusqlite::types::Value::Integer(id)))
            {
                label.2 += 1;
            }
        }
    }
    let mut tags = Vec::new();
    for (_, name, count) in labels {
        if name != Value::String(String::new()) {
            tags.extend(std::iter::repeat_n(name, count));
        }
    }
    Ok(tags)
}

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

#[cfg(test)]
#[path = "contact_metadata_tests.rs"]
mod tests;
