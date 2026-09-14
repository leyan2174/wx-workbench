//! Bounded WeChat label decoding, preserving legacy association multiplicity.
use crate::business::contacts::{ContactId, Error as ContactError, Tag, TagMember};
use crate::toolkit::contact_metadata::{
    extract_field_30 as field_30, parse_label_id as label_id, sqlite_id_equal as id_equal,
};
use anyhow::{bail, Result};
use rusqlite::{
    types::{Value as SqlValue, ValueRef},
    Connection, OpenFlags,
};
use std::{collections::HashMap, path::Path};
pub(crate) const MAX_LABELS: usize = 10_000;
pub(crate) const MAX_ASSOCIATIONS: usize = 100_000;
pub(crate) const MAX_BUFFER_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 4096;
pub(crate) const MAX_RESULT_TEXT_BYTES: usize = 16 * 1024 * 1024;

fn budget(valid: bool, diagnostic: &'static str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(anyhow::Error::new(ContactError::Limit).context(diagnostic))
    }
}

fn check_text(value: ValueRef<'_>) -> Result<()> {
    if let ValueRef::Text(bytes) | ValueRef::Blob(bytes) = value {
        budget(
            bytes.len() <= MAX_TEXT_BYTES,
            "contact text byte limit exceeded",
        )?;
    }
    Ok(())
}

fn add_text(total: &mut usize, bytes: usize) -> Result<()> {
    budget(
        bytes <= MAX_RESULT_TEXT_BYTES - *total,
        "contact result text byte limit exceeded",
    )?;
    *total += bytes;
    Ok(())
}

/// 一次读取事务内加载定义与关联；不把坏库、缺表或损坏行当成成功空结果。
pub(crate) fn read_tags(path: &Path, names: &HashMap<String, String>) -> Result<Vec<Tag>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| ContactError::Unavailable)?;
    let tx = conn.unchecked_transaction()?;
    let schema = super::columns(&tx, "contact_label")?;
    if !super::has(&schema, &["label_id_", "label_name_", "sort_order_"]) {
        return Err(crate::business::contacts::Error::Unsupported("contact labels").into());
    }
    // 排序与复制前限制定义行数；重复 ID 也占限额，不能绕过。
    let count: usize = tx.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM contact_label LIMIT ?1)",
        [MAX_LABELS + 1],
        |row| row.get(0),
    )?;
    budget(count <= MAX_LABELS, "contact label limit exceeded")?;
    let mut stmt = tx.prepare(
        "SELECT label_id_, label_name_, sort_order_ FROM contact_label ORDER BY sort_order_",
    )?;
    let mut rows = stmt.query([])?;
    let mut labels: Vec<(SqlValue, i64, Tag)> = Vec::new();
    let mut text_bytes = 0;
    while let Some(row) = rows.next()? {
        check_text(row.get_ref(0)?)?;
        check_text(row.get_ref(1)?)?;
        let id = row.get::<_, SqlValue>(0)?;
        let name: String = row.get(1)?;
        let order: i64 = row.get(2)?;
        add_text(&mut text_bytes, name.len())?;
        let tag = Tag {
            name,
            members: vec![],
        };
        // 与旧 Python dict 一致：重复 ID 更新定义，但保持首次插入位置。
        if let Some(old) = labels.iter_mut().find(|(old, _, _)| id_equal(old, &id)) {
            old.1 = order;
            old.2 = tag;
        } else {
            labels.push((id, order, tag));
        }
    }
    let mut total_associations = 0;
    if !labels.is_empty() {
        let columns = super::columns(&tx, "contact")?;
        if !super::has(&columns, &["username", "extra_buffer"]) {
            return Err(ContactError::Unsupported("contact label membership").into());
        }
        let mut stmt = tx
            .prepare("SELECT username, extra_buffer FROM contact WHERE extra_buffer IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            // 借用 SQLite 内存先检查长度，不复制未验证的大 BLOB。
            let buffer = match row.get_ref(1)? {
                ValueRef::Blob(bytes) => {
                    budget(
                        bytes.len() <= MAX_BUFFER_BYTES,
                        "contact buffer byte limit exceeded",
                    )?;
                    bytes
                }
                ValueRef::Text([]) => continue,
                ValueRef::Integer(0) | ValueRef::Real(0.0) => continue,
                _ => bail!("unsupported contact extra_buffer type"),
            };
            let Some(ids) = field_30(buffer) else {
                continue;
            };
            check_text(row.get_ref(0)?)?;
            let username: &str = row.get_ref(0)?.as_str()?;
            let display_name = names.get(username).map(String::as_str).unwrap_or(username);
            budget(
                display_name.len() <= MAX_TEXT_BYTES,
                "contact display name byte limit exceeded",
            )?;
            for id in ids.split(',').filter_map(label_id) {
                if let Some((_, _, tag)) = labels
                    .iter_mut()
                    .find(|(old, _, _)| id_equal(old, &SqlValue::Integer(id)))
                {
                    budget(
                        total_associations < MAX_ASSOCIATIONS,
                        "contact association limit exceeded",
                    )?;
                    add_text(&mut text_bytes, username.len() + display_name.len())?;
                    // 重复关联照旧计数；所有限额检查先于复制与追加。
                    tag.members.push(TagMember {
                        id: ContactId(username.into()),
                        display_name: display_name.into(),
                    });
                    total_associations += 1;
                }
            }
        }
    }
    labels.sort_by_key(|(_, order, _)| *order);
    let tags: Vec<_> = labels.into_iter().map(|(_, _, tag)| tag).collect();
    Ok(tags)
}
