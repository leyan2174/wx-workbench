//! 详细聊天目录查询：独立枚举实际消息表，已映射聊天复用紧凑导出的分片引擎。
//! 不发现账号、不写导出文件，也不把解压失败降级为部分成功。
use super::{ensure_complete_message_inventory, export, DbCache, Names};
use anyhow::{ensure, Result};
use serde_json::Value;
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

pub(super) fn contact_alias(path: &Path, username: &str) -> Result<Value> {
    Ok(
        crate::adapters::wechat::contacts::export_alias::read(path, username)?
            .map(Value::String)
            .unwrap_or(Value::Null),
    )
}
