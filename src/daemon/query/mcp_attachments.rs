//! 文件与合并转发附件的只读适配；复用严格消息定位，不接收任意缓存根或输出路径。
use super::{ensure_complete_message_inventory, strict_message, DbCache, Names};
use crate::toolkit::attachment_refs::{self, ErrorKind, Kind, MessageInput};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

pub async fn q_attachment_reference(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    item_index: Option<i64>,
) -> Result<Value> {
    ensure!(local_id > 0, "local_id must be positive");
    ensure!(
        item_index.is_none_or(|index| index >= 0),
        "invalid item index"
    );
    use strict_message::Resolution;
    let message = match strict_message::locate(db, names, chat, local_id, create_time).await? {
        Resolution::Found(message) => message,
        Resolution::ChatNotFound => return Ok(failure(1, "chat not found")),
        Resolution::MessageNotFound => return Ok(failure(1, "message not found")),
        Resolution::AmbiguousChat => return Ok(failure(2, "ambiguous chat")),
        Resolution::AmbiguousMessage => return Ok(failure(2, "ambiguous message identity")),
    };
    let base_type = if message.kind > u32::MAX as i64 {
        message.kind & 0xffff_ffff
    } else {
        message.kind
    };
    if base_type != 49 {
        return Ok(failure(1, "expected app message base_type=49"));
    }
    // 根只来自已固定账号的配置，不从消息 XML 或工具参数推断另一个账号。
    let base = db
        .db_dir()
        .parent()
        .context("missing account root")?
        .to_path_buf();
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let limit = if item_index.is_some() {
            500_000
        } else {
            20_000
        };
        let bytes = message.bounded_decode(limit)?;
        let body = std::str::from_utf8(&bytes).context("invalid attachment text encoding")?;
        let input = MessageInput {
            username: &message.username,
            source: &message.source,
            local_id: message.local_id,
            create_time: message.create_time,
            body,
        };
        let metadata = match item_index {
            Some(index) => attachment_refs::parse_record_item(&input, index),
            None => attachment_refs::parse_file_message(&input),
        };
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => return Ok(attachment_error(error)),
        };
        let reference = match attachment_refs::find_reference(&base, &metadata) {
            Ok(reference) => reference,
            Err(error) => return Ok(attachment_error(error)),
        };
        let status = match (&reference, metadata.kind) {
            (Some(_), _) => "found",
            (None, Kind::Text) => "text",
            (None, Kind::MetadataOnly) => "metadata_only",
            (None, _) => "missing",
        };
        if let Some(reference) = &reference {
            ensure!(
                reference.file().metadata()?.len() == reference.size,
                "attachment changed"
            );
        }
        // 序列化期间保留引用的只读句柄；返回路径不是永久锁或不可变文件凭证。
        Ok(json!({
            "exit_code": 0, "status": status, "metadata": metadata,
            "reference": reference,
        }))
    })
    .await?;
    ensure_complete_message_inventory(db, names)?;
    result
}

fn failure(code: i32, text: &str) -> Value {
    json!({"exit_code":code,"text":text})
}

fn attachment_error(error: attachment_refs::Error) -> Value {
    let code = if error.kind == ErrorKind::Ambiguous {
        2
    } else {
        1
    };
    // 错误仅由模块的固定类别和阶段构成，不包含原始 XML、路径或凭据。
    failure(code, &error.to_string())
}
