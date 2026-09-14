//! MCP 联系人标签只读查询；账号由调用方传入，不发现账号或回退其他副本。
use crate::daemon::cache::DbCache;
use crate::toolkit::contact_metadata::{
    extract_field_30 as field_30, parse_label_id as label_id, sqlite_id_equal as id_equal,
};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{
    types::{Value as SqlValue, ValueRef},
    Connection, OpenFlags,
};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

const MAX_LABELS: usize = 10_000;
const MAX_ASSOCIATIONS: usize = 100_000;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_RESULT_TEXT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagMember {
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactTag {
    pub name: String,
    pub member_count: usize,
    pub members: Vec<TagMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactTags {
    pub total_tags: usize,
    pub total_associations: usize,
    pub tags: Vec<ContactTag>,
}

pub async fn q_contact_tags(db: &DbCache, names: &HashMap<String, String>) -> Result<ContactTags> {
    // 两种历史分隔符均只向同一账号缓存查询，不枚举磁盘或读取配置。
    let path = match db.get("contact/contact.db").await? {
        Some(path) => path,
        None => db
            .get("contact\\contact.db")
            .await?
            .context("contact database unavailable")?,
    };
    contact_tags_from_path(&path, names)
}

pub async fn q_tag_members(
    db: &DbCache,
    names: &HashMap<String, String>,
    tag_name: &str,
) -> Result<ContactTag> {
    validate_query(tag_name)?;
    select_tag(&q_contact_tags(db, names).await?, tag_name).cloned()
}

/// 大小写不敏感的精确匹配优先；重复精确名称也拒绝，不任取首项。
pub fn select_tag<'a>(tags: &'a ContactTags, query: &str) -> Result<&'a ContactTag> {
    validate_query(query)?;
    let query = query.trim().to_lowercase();
    let exact: Vec<_> = tags
        .tags
        .iter()
        .filter(|t| t.name.to_lowercase() == query)
        .collect();
    let matches = if exact.is_empty() {
        tags.tags
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&query))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [tag] => Ok(tag),
        [] => bail!("tag not found"),
        _ => bail!("ambiguous tag name: {} matches", matches.len()),
    }
}

fn validate_query(query: &str) -> Result<()> {
    // 检查原始 UTF-8 字节数，不能先 trim/lowercase 或读取缓存。
    ensure!(
        query.len() <= MAX_TEXT_BYTES,
        "tag query byte limit exceeded"
    );
    Ok(())
}

fn check_text(value: ValueRef<'_>) -> Result<()> {
    if let ValueRef::Text(bytes) | ValueRef::Blob(bytes) = value {
        ensure!(
            bytes.len() <= MAX_TEXT_BYTES,
            "contact text byte limit exceeded"
        );
    }
    Ok(())
}

fn add_text(total: &mut usize, bytes: usize) -> Result<()> {
    ensure!(
        bytes <= MAX_RESULT_TEXT_BYTES - *total,
        "contact result text byte limit exceeded"
    );
    *total += bytes;
    Ok(())
}

/// 一次读取事务内加载定义与关联；不把坏库、缺表或损坏行当成成功空结果。
pub fn contact_tags_from_path(path: &Path, names: &HashMap<String, String>) -> Result<ContactTags> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tx = conn.unchecked_transaction()?;
    // 排序与复制前限制定义行数；重复 ID 也占限额，不能绕过。
    let count: usize = tx.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM contact_label LIMIT ?1)",
        [MAX_LABELS + 1],
        |row| row.get(0),
    )?;
    ensure!(count <= MAX_LABELS, "contact label limit exceeded");
    let mut stmt = tx.prepare(
        "SELECT label_id_, label_name_, sort_order_ FROM contact_label ORDER BY sort_order_",
    )?;
    let mut rows = stmt.query([])?;
    let mut labels: Vec<(SqlValue, i64, ContactTag)> = Vec::new();
    let mut text_bytes = 0;
    while let Some(row) = rows.next()? {
        check_text(row.get_ref(0)?)?;
        check_text(row.get_ref(1)?)?;
        let id = row.get::<_, SqlValue>(0)?;
        let name: String = row.get(1)?;
        let order: i64 = row.get(2)?;
        add_text(&mut text_bytes, name.len())?;
        let tag = ContactTag {
            name,
            member_count: 0,
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
        let mut stmt = tx
            .prepare("SELECT username, extra_buffer FROM contact WHERE extra_buffer IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            // 借用 SQLite 内存先检查长度，不复制未验证的大 BLOB。
            let buffer = match row.get_ref(1)? {
                ValueRef::Blob(bytes) => {
                    ensure!(
                        bytes.len() <= MAX_BUFFER_BYTES,
                        "contact buffer byte limit exceeded"
                    );
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
            ensure!(
                display_name.len() <= MAX_TEXT_BYTES,
                "contact display name byte limit exceeded"
            );
            for id in ids.split(',').filter_map(label_id) {
                if let Some((_, _, tag)) = labels
                    .iter_mut()
                    .find(|(old, _, _)| id_equal(old, &SqlValue::Integer(id)))
                {
                    ensure!(
                        total_associations < MAX_ASSOCIATIONS,
                        "contact association limit exceeded"
                    );
                    add_text(&mut text_bytes, username.len() + display_name.len())?;
                    // 重复关联照旧计数；所有限额检查先于复制与追加。
                    tag.members.push(TagMember {
                        username: username.into(),
                        display_name: display_name.into(),
                    });
                    tag.member_count += 1;
                    total_associations += 1;
                }
            }
        }
    }
    labels.sort_by_key(|(_, order, _)| *order);
    let tags: Vec<_> = labels.into_iter().map(|(_, _, tag)| tag).collect();
    Ok(ContactTags {
        total_tags: tags.len(),
        total_associations,
        tags,
    })
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-contacts/tests.rs"]
mod tests;
