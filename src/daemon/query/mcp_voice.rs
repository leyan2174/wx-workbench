//! 只读 VoiceInfo 元数据查询；不读取音频 BLOB，不从消息 type 推断大小。
use crate::daemon::cache::DbCache;
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct VoiceQuery {
    pub username: String,
    pub limit: usize,
    pub offset: usize,
    pub since: Option<i64>,
    pub until: Option<i64>,
}
impl VoiceQuery {
    fn candidate_limit(&self) -> Result<usize> {
        ensure!(
            !self.username.trim().is_empty(),
            "explicit username required"
        );
        ensure!(self.limit > 0, "limit must be positive");
        ensure!(
            !matches!((self.since, self.until), (Some(a), Some(b)) if a > b),
            "since exceeds until"
        );
        let count = self
            .offset
            .checked_add(self.limit)
            .context("pagination overflow")?;
        ensure!(
            count <= i64::MAX as usize,
            "pagination exceeds SQLite integer range"
        );
        Ok(count)
    }
}

#[derive(Debug, Clone)]
pub struct MediaShard {
    pub source: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceMessage {
    pub username: String,
    pub source: String,
    pub chat_name_id: i64,
    pub media_rowid: i64,
    pub local_id: i64,
    pub create_time: i64,
    /// SQL NULL 保持未知；0 字节保持 0，不伪造音频大小。
    pub voice_data_bytes: Option<u64>,
}

pub fn resolve_exact_chat(chat: &str, names: &HashMap<String, String>) -> Result<String> {
    ensure!(!chat.trim().is_empty(), "chat must not be empty");
    if names.contains_key(chat) {
        return Ok(chat.into());
    }
    let matches: Vec<_> = names
        .iter()
        .filter(|(_, display)| display.as_str() == chat)
        .map(|(user, _)| user)
        .collect();
    ensure!(
        matches.len() == 1,
        "chat has no unique exact match; use explicit username"
    );
    Ok(matches[0].clone())
}

fn source_key(source: &str) -> Result<String> {
    let source = source.replace('\\', "/").to_ascii_lowercase();
    let number = source
        .strip_prefix("message/media_")
        .and_then(|s| s.strip_suffix(".db"))
        .context("expected message/media_N.db source")?;
    ensure!(
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        "invalid media shard source"
    );
    Ok(source)
}

/// 从账号级 DbCache 只读枚举原始媒体键；不读取密钥文件。
/// 磁盘上额外媒体片或 get 返回 None 均失败，不返回已查询的部分结果。
pub async fn q_voice_messages(db: &DbCache, query: &VoiceQuery) -> Result<Vec<VoiceMessage>> {
    query.candidate_limit()?;
    let media_keys = db.media_db_keys();
    // DbCache 精确匹配原始键；只规范化证据，不能改变传给 get 的键。
    let mut original_keys = BTreeMap::new();
    for original in &media_keys {
        ensure!(
            original_keys
                .insert(source_key(original)?, original)
                .is_none(),
            "duplicate canonical media key"
        );
    }
    let keys: BTreeSet<_> = original_keys.keys().cloned().collect();
    ensure!(
        !keys.is_empty() && keys.len() == media_keys.len(),
        "empty or duplicate media inventory"
    );
    let discovered = discover_media(db.db_dir())?;
    ensure!(
        discovered == keys,
        "unknown or missing media shard; complete account inventory required"
    );
    let mut shards = Vec::new();
    for (source, original) in original_keys {
        let path = db
            .get(original)
            .await
            .with_context(|| format!("resolve media shard {source}"))?
            .with_context(|| format!("media shard unavailable or undecrypted: {source}"))?;
        shards.push(MediaShard { source, path });
    }
    let rows = query_voice_shards(&shards, query)?;
    ensure!(
        discover_media(db.db_dir())? == discovered,
        "media inventory changed during query"
    );
    Ok(rows)
}

fn discover_media(root: &Path) -> Result<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    for entry in std::fs::read_dir(root.join("message")).context("enumerate media directory")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if let Ok(source) = source_key(&format!("message/{name}")) {
            ensure!(
                entry.file_type()?.is_file(),
                "media shard is not a regular file"
            );
            keys.insert(source);
        }
    }
    Ok(keys)
}

fn validate_rowid_schema(conn: &Connection, table: &str) -> Result<()> {
    let mut tables = conn.prepare(
        "SELECT type, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1 COLLATE NOCASE",
    )?;
    let found = tables
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        found.len() == 1 && found[0].0 == "table" && found[0].1 == 0,
        "unsupported rowid schema: {table} must be an ordinary rowid table"
    );
    // xinfo 包含生成/隐藏列，避免 table_info 漏检；不挑一个未遮蔽别名继续猜。
    let mut columns = conn.prepare("SELECT name FROM pragma_table_xinfo(?1, 'main')")?;
    let mut rows = columns.query([table])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        ensure!(
            !["rowid", "_rowid_", "oid"]
                .iter()
                .any(|alias| name.eq_ignore_ascii_case(alias)),
            "unsupported rowid schema: {table} shadows SQLite rowid"
        );
    }
    Ok(())
}

/// 显式离线清单入口；调用方保证清单完整且属于同一账号，不发现私人配置。
pub fn query_voice_shards(shards: &[MediaShard], query: &VoiceQuery) -> Result<Vec<VoiceMessage>> {
    let count = query.candidate_limit()?;
    ensure!(!shards.is_empty(), "no media shards supplied");
    let mut sources = BTreeSet::new();
    let mut paths = Vec::new();
    let mut result = Vec::new();
    for shard in shards {
        let source = source_key(&shard.source)?;
        ensure!(sources.insert(source.clone()), "duplicate media source");
        ensure!(shard.path.is_file(), "media shard unavailable: {source}");
        for path in &paths {
            ensure!(
                !same_file::is_same_file(path, &shard.path)?,
                "media sources alias the same file"
            );
        }
        paths.push(shard.path.clone());
        (|| -> Result<()> {
            let conn = Connection::open_with_flags(
                &shard.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| format!("open media shard {source}"))?;
            conn.busy_timeout(std::time::Duration::from_secs(2))?;
            let tx = conn.unchecked_transaction()?;
            validate_rowid_schema(&tx, "Name2Id")?;
            validate_rowid_schema(&tx, "VoiceInfo")?;
            // 先校验 VoiceInfo 表，即使本片没有目标联系人也不隐藏损坏模式。
            let mut voices = tx
                .prepare(
                    "SELECT rowid, local_id, create_time, length(voice_data), typeof(voice_data)
            FROM VoiceInfo WHERE chat_name_id = ?1 AND (?2 IS NULL OR create_time >= ?2)
            AND (?3 IS NULL OR create_time <= ?3)
            ORDER BY create_time DESC, local_id DESC, rowid DESC LIMIT ?4",
                )
                .with_context(|| format!("invalid VoiceInfo schema: {source}"))?;
            let mut names =
                tx.prepare("SELECT rowid FROM Name2Id WHERE user_name = ?1 COLLATE BINARY")?;
            let ids = names
                .query_map([&query.username], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(ids.len() <= 1, "ambiguous Name2Id ownership: {source}");
            if let Some(id) = ids.first() {
                let mut rows = voices.query(rusqlite::params![
                    id,
                    query.since,
                    query.until,
                    count as i64
                ])?;
                while let Some(row) = rows.next()? {
                    let kind: String = row.get(4)?;
                    ensure!(
                        kind == "blob" || kind == "null",
                        "voice_data is not BLOB/NULL: {source}"
                    );
                    let size: Option<i64> = row.get(3)?;
                    ensure!(size.is_none_or(|n| n >= 0), "invalid voice byte length");
                    result.push(VoiceMessage {
                        username: query.username.clone(),
                        source: source.clone(),
                        chat_name_id: *id,
                        media_rowid: row.get(0)?,
                        local_id: row.get(1)?,
                        create_time: row.get(2)?,
                        voice_data_bytes: size.map(|n| n as u64),
                    });
                }
            }
            Ok(())
        })()
        .with_context(|| format!("query media shard {source}"))?;
        // 每片最多取全局页末所需候选；阶段性截断不影响最终前 count 项。
        result.sort_by(|a, b| {
            b.create_time
                .cmp(&a.create_time)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| b.local_id.cmp(&a.local_id))
                .then_with(|| b.media_rowid.cmp(&a.media_rowid))
        });
        result.truncate(count);
    }
    Ok(result
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .collect())
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-voice/tests.rs"]
mod tests;
