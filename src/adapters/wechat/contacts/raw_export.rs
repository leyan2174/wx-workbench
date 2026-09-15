//! Explicit legacy raw-export projection; dynamic values are not domain contacts.
//! 旧 export_all_chats 联系人元数据查询；不发现账号、不写数据库或导出文件。
use super::label_values::{extract_field_30, parse_label_id, sqlite_id_equal};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Default)]
pub struct RawContactMetadata {
    pub fields: Map<String, Value>,
    /// 兼容回退的原因；不应合并进旧导出字段。
    pub diagnostics: Vec<String>,
}

/// 群聊直接省略元数据，甚至不打开路径。其他情况只读打开指定数据库。
pub fn contact_metadata_for_export(
    contact_db: &Path,
    username: &str,
    is_group: bool,
) -> RawContactMetadata {
    if is_group {
        return RawContactMetadata::default();
    }
    let mut result = RawContactMetadata::default();
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

#[cfg(test)]
mod tests;
