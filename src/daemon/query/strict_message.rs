//! 引用与附件查询共享的严格消息定位，仅使用显式账号缓存。
use super::{ensure_complete_message_inventory, msg_table_re, DbCache, Names};
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags, OptionalExtension};
use std::{io::Read, path::PathBuf};

pub(super) const MAX_STORED_BYTES: usize = 1_048_576;

pub(super) enum Resolution {
    ChatNotFound,
    AmbiguousChat,
    MessageNotFound,
    AmbiguousMessage,
    Found(StrictMessage),
}

pub(super) struct StrictMessage {
    pub(super) username: String,
    pub(super) source: String,
    pub(super) local_id: i64,
    pub(super) create_time: i64,
    pub(super) kind: i64,
    compression: Option<i64>,
    content: Option<(bool, Vec<u8>)>,
}

impl StrictMessage {
    /// 对压缩和未压缩消息体统一限制解码字节数。
    /// 独立的存储字节上限固定为 1 MiB，不随调用方限制调整。
    pub(super) fn bounded_decode(&self, limit: usize) -> Result<Vec<u8>> {
        let (blob, bytes) = self.content.as_ref().context("empty message content")?;
        if *blob && self.compression == Some(4) {
            let read_limit = u64::try_from(limit)?
                .checked_add(1)
                .context("decoded byte limit overflow")?;
            let mut decoded = Vec::new();
            zstd::stream::read::Decoder::new(bytes.as_slice())?
                .take(read_limit)
                .read_to_end(&mut decoded)?;
            ensure!(
                decoded.len() <= limit,
                "message body exceeds decoded byte limit"
            );
            Ok(decoded)
        } else {
            ensure!(
                bytes.len() <= limit,
                "message body exceeds decoded byte limit"
            );
            Ok(bytes.clone())
        }
    }
}

/// 先确定消息身份唯一，再由调用方检查消息类型和解码消息体。
/// create_time 为零时不按时间筛选；完整性错误直接传播。
/// 数据库查询返回错误时，也会执行查询后的完整清单检查。
pub(super) async fn locate(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
) -> Result<Resolution> {
    let username = match resolve_unique_username(chat, names) {
        ChatResolution::Unique(username) => username,
        ChatResolution::NotFound => return Ok(Resolution::ChatNotFound),
        ChatResolution::Ambiguous => return Ok(Resolution::AmbiguousChat),
    };
    ensure_complete_message_inventory(db, names)?;
    let mut keys = names.msg_db_keys.clone();
    keys.sort();
    keys.dedup();
    let mut shards = Vec::new();
    for key in keys {
        let path = db
            .get(&key)
            .await?
            .with_context(|| format!("unavailable message shard: {key}"))?;
        shards.push((key, path));
    }
    let result =
        tokio::task::spawn_blocking(move || lookup(&shards, &username, local_id, create_time))
            .await?;
    // 读取期间可能出现新分片；交付结果前再次检查，不能把旧快照误报为唯一命中。
    ensure_complete_message_inventory(db, names)?;
    result
}

enum ChatResolution {
    NotFound,
    Unique(String),
    Ambiguous,
}

fn resolve_unique_username(chat: &str, names: &Names) -> ChatResolution {
    if chat.trim().is_empty() {
        return ChatResolution::NotFound;
    }
    // 精确账号优先；沿用显式 wxid 和群账号入口，以支持联系人表尚未收录的会话。
    if names.map.contains_key(chat) || chat.starts_with("wxid_") || chat.contains("@chatroom") {
        return ChatResolution::Unique(chat.to_owned());
    }
    let lower = chat.to_lowercase();
    for exact in [true, false] {
        let mut candidates = names.map.iter().filter(|(_, display)| {
            let display = display.to_lowercase();
            if exact {
                display == lower
            } else {
                display.contains(&lower)
            }
        });
        if let Some((username, _)) = candidates.next() {
            return if candidates.next().is_some() {
                ChatResolution::Ambiguous
            } else {
                ChatResolution::Unique(username.clone())
            };
        }
    }
    ChatResolution::NotFound
}

fn lookup(
    shards: &[(String, PathBuf)],
    username: &str,
    local_id: i64,
    create_time: i64,
) -> Result<Resolution> {
    let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
    ensure!(msg_table_re().is_match(&table), "invalid message table");
    let mut matched = None;
    let mut ambiguous = false;
    for (source, path) in shards {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("cannot read message shard: {source}"))?;
        let conn = conn.unchecked_transaction()?;
        let schema: Option<(String, i64)> = conn.query_row(
            "SELECT type, wr FROM pragma_table_list WHERE schema='main' AND name=?1 COLLATE NOCASE",
            [&table],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        // 只允许跳过确实不存在的目标；不支持的同名对象不能隐藏潜在重复消息。
        let Some((kind, without_rowid)) = schema else {
            continue;
        };
        ensure!(
            kind == "table" && without_rowid == 0,
            "unsupported message table schema"
        );
        let mut statement = conn.prepare(&format!(
            "SELECT local_type,create_time,WCDB_CT_message_content,length(CAST(message_content AS BLOB)),message_content FROM [{table}] WHERE local_id=?1 AND (?2=0 OR create_time=?2) LIMIT 2"
        ))?;
        let mut rows = statement.query([local_id, create_time])?;
        while let Some(row) = rows.next()? {
            if matched.is_some() {
                ambiguous = true;
                continue;
            }
            let kind: i64 = row.get(0)?;
            let timestamp: i64 = row.get(1)?;
            let compression: Option<i64> = row.get(2)?;
            let length: Option<i64> = row.get(3)?;
            ensure!(
                length.unwrap_or(0) <= MAX_STORED_BYTES as i64,
                "message body exceeds stored byte limit"
            );
            let content = match row.get_ref(4)? {
                ValueRef::Text(bytes) => {
                    std::str::from_utf8(bytes).context("SQLite TEXT is not valid UTF-8")?;
                    Some((false, bytes.to_vec()))
                }
                ValueRef::Blob(bytes) => Some((true, bytes.to_vec())),
                ValueRef::Null => None,
                _ => anyhow::bail!("message body must be SQLite TEXT, BLOB or NULL"),
            };
            matched = Some((source.clone(), kind, timestamp, compression, content));
        }
    }
    if ambiguous {
        return Ok(Resolution::AmbiguousMessage);
    }
    let Some((source, kind, timestamp, compression, content)) = matched else {
        return Ok(Resolution::MessageNotFound);
    };
    Ok(Resolution::Found(StrictMessage {
        username: username.to_owned(),
        source,
        local_id,
        create_time: timestamp,
        kind,
        compression,
        content,
    }))
}
