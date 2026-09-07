//! 详细聊天目录查询：独立枚举实际消息表，已映射聊天复用紧凑导出的分片引擎。
//! 不发现账号、不写导出文件，也不把解压失败降级为部分成功。
use super::{ensure_complete_message_inventory, export, DbCache, Names};
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags, Row};
use serde_json::{Map, Value};
use std::path::Path;

mod catalog;

/// 目录导出独立枚举实际消息表；不改变基于 SessionTable 的 q_export_chat_list。
pub async fn q_export_directory_catalog(db: &DbCache, names: &Names) -> Result<Value> {
    let catalog = catalog::load(db, names).await?;
    Ok(serde_json::json!({"catalog":"message_tables", "chats":catalog.entries}))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportShape {
    Compact,
    Directory,
}

/// 镜像 q_export_username 的签名，使用服务器已固定的同账号 Names 和分片清单。
/// 真实 username 按原字节计算表名；目录报告的完整 unknown_ ID 单独解析到原表。
/// 不 trim，不把别名或显示名解析成另一个联系人。
pub async fn q_export_directory_by_username(
    db: &DbCache,
    names: &Names,
    username: String,
) -> Result<Value> {
    ensure!(!username.is_empty(), "username 不能为空");
    ensure_complete_message_inventory(db, names)?;
    let result = if username.starts_with("unknown_") {
        let catalog = catalog::load(db, names).await?;
        if catalog
            .entries
            .iter()
            .any(|entry| entry.identity_status == "unmapped" && entry.target.username == username)
        {
            catalog::export_unmapped(db, names, catalog, username).await
        } else {
            export::q_export_username_with_shape(db, names, username, ExportShape::Directory).await
        }
    } else {
        export::q_export_username_with_shape(db, names, username, ExportShape::Directory).await
    };
    // 读取期间新增或无法枚举分片时不能交付自称完整的目录文档。
    ensure_complete_message_inventory(db, names)?;
    result
}

pub(super) fn detail_projection(conn: &Connection, table: &str) -> Result<String> {
    ensure!(super::msg_table_re().is_match(table), "消息表名不合法");
    let columns = conn
        .prepare(&format!("PRAGMA table_info([{table}])"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // 缺列投影 NULL，不能以 0 伪造未知字段；现有列仍由 SQLite 保留原始存储类型。
    Ok(["server_id", "sort_seq", "status"]
        .into_iter()
        .map(|name| {
            if columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case(name))
            {
                format!(",[{name}]")
            } else {
                format!(",NULL AS [{name}]")
            }
        })
        .collect())
}

fn server_id(value: ValueRef<'_>) -> Result<Value> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::from(value),
        // 保留 TEXT 的前导零、超 i64 数字及空字符串；不经浮点或强制数值转换。
        ValueRef::Text(bytes) => Value::String(std::str::from_utf8(bytes)?.into()),
        _ => anyhow::bail!("server_id 必须为 SQLite INTEGER、TEXT 或 NULL"),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_details(
    extras: &mut Map<String, Value>,
    row: &Row<'_>,
    local_type: i64,
    mapped: &str,
    group_prefix: &str,
    is_group: bool,
    chat_username: &str,
    raw_content: Option<&str>,
) -> Result<()> {
    let sender = if is_group && (mapped.is_empty() || mapped == chat_username) {
        group_prefix
    } else {
        mapped
    };
    // 不从显示名或“me”反推 username；群前缀只在分片映射缺失时回退。
    let sender = (!sender.is_empty()).then_some(sender);
    extras.insert("local_type".into(), Value::from(local_type));
    extras.insert("server_id".into(), server_id(row.get_ref(6)?)?);
    extras.insert(
        "sort_seq".into(),
        serde_json::to_value(row.get::<_, Option<i64>>(7)?)?,
    );
    extras.insert(
        "status".into(),
        serde_json::to_value(row.get::<_, Option<i64>>(8)?)?,
    );
    extras.insert("sender_username".into(), serde_json::to_value(sender)?);
    // 原文包括群聊前缀，不使用正文提取后的摘要；NULL 与空文本分别序列化。
    extras.insert("raw_content".into(), serde_json::to_value(raw_content)?);
    Ok(())
}

pub(super) fn contact_alias(path: &Path, username: &str) -> Result<Value> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let snapshot = conn.unchecked_transaction()?;
    let columns = snapshot
        .prepare("PRAGMA table_info(contact)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case("alias"))
    {
        return Ok(Value::Null);
    }
    // 与紧凑元数据保持相同的本地类型过滤及首行规则，额外固定 username 为精确匹配。
    let filter = if columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case("local_type"))
    {
        " AND local_type != 3"
    } else {
        ""
    };
    let mut statement = snapshot.prepare(&format!(
        "SELECT alias FROM contact WHERE username COLLATE BINARY = ?1{filter} LIMIT 1"
    ))?;
    let mut rows = statement.query([username])?;
    let Some(row) = rows.next()? else {
        return Ok(Value::Null);
    };
    match row.get_ref(0)? {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Text(bytes) => Ok(Value::String(
            std::str::from_utf8(bytes)
                .context("contact.alias 不是有效 UTF-8")?
                .into(),
        )),
        _ => anyhow::bail!("contact.alias 必须为 SQLite TEXT 或 NULL"),
    }
}
