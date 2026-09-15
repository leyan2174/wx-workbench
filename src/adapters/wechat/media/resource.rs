//! Validated WeChat resource-table reads; no decoding or publication.

/// Host cache request; the physical WeChat source name is adapter-owned.
pub const fn source_key() -> &'static str {
    "message/message_resource.db"
}
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::{fs, path::Path};

const MAX_PACKED_BYTES: i64 = 1024 * 1024;

/// 此结构不是认证凭证。source 为调用者已核验的逻辑消息分片名，不用于打开文件。
#[derive(Debug, Clone, Serialize)]
pub struct MessageIdentity {
    pub username: String,
    pub source: String,
    pub local_id: i64,
    pub create_time: i64,
    pub local_type: i64,
}

pub(crate) fn no_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        ensure!(
            matches!(fs::symlink_metadata(Path::new(&name)), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
            "static resource snapshot required: sidecar present or unreadable"
        );
    }
    Ok(())
}

fn real_rowid_table(conn: &Connection, table: &str) -> Result<()> {
    let (kind, wr): (String, i64) = conn.query_row(
        "SELECT type, wr FROM pragma_table_list WHERE schema='main' AND name=?1 COLLATE NOCASE",
        [table],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        kind == "table" && wr == 0,
        "unsupported resource table schema"
    );
    let shadows: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_xinfo(?1) WHERE lower(name) IN ('rowid','_rowid_','oid')",
        [table], |r| r.get(0))?;
    ensure!(shadows == 0, "shadowed resource rowid");
    Ok(())
}

pub(crate) enum ResourceLookup {
    Found(i64, String),
    Missing,
    Ambiguous,
    Md5Missing,
}

pub(crate) struct ResourceReader(Connection);

impl ResourceReader {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        no_sidecars(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN;")?;
        real_rowid_table(&conn, "ChatName2Id")?;
        real_rowid_table(&conn, "MessageResourceInfo")?;
        Ok(Self(conn))
    }

    pub(crate) fn lookup(&self, identity: &MessageIdentity) -> Result<ResourceLookup> {
        let conn = &self.0;
        let mut statement = conn
            .prepare("SELECT rowid FROM ChatName2Id WHERE user_name=?1 COLLATE BINARY LIMIT 2")?;
        let ids = statement
            .query_map([&identity.username], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if ids.is_empty() {
            return Ok(ResourceLookup::Missing);
        }
        if ids.len() != 1 {
            return Ok(ResourceLookup::Ambiguous);
        }
        // 不使用最新时间回退，也不将不同高位类型标志视为同一消息。
        let mut statement = conn.prepare("SELECT rowid, length(packed_info), typeof(packed_info),
        typeof(chat_id), typeof(message_local_id), typeof(message_local_type), typeof(message_create_time)
        FROM MessageResourceInfo WHERE chat_id=?1 AND message_local_id=?2
        AND message_local_type=?3 AND message_create_time=?4 LIMIT 2")?;
        let mut rows = statement.query(params![
            ids[0],
            identity.local_id,
            identity.local_type,
            identity.create_time
        ])?;
        let Some(row) = rows.next()? else {
            return Ok(ResourceLookup::Missing);
        };
        let rowid: i64 = row.get(0)?;
        let size: i64 = row.get(1)?;
        ensure!(
            (1..=MAX_PACKED_BYTES).contains(&size),
            "packed_info size limit exceeded"
        );
        ensure!(
            row.get::<_, String>(2)? == "blob",
            "packed_info must be BLOB"
        );
        for i in 3..7 {
            ensure!(
                row.get::<_, String>(i)? == "integer",
                "resource identity must use INTEGER values"
            );
        }
        if rows.next()?.is_some() {
            return Ok(ResourceLookup::Ambiguous);
        }
        let blob: Vec<u8> = conn.query_row(
            "SELECT packed_info FROM MessageResourceInfo WHERE rowid=?1",
            [rowid],
            |r| r.get(0),
        )?;
        Ok(match extract_md5_from_packed_info(&blob) {
            Some(md5) => ResourceLookup::Found(rowid, md5),
            None => ResourceLookup::Md5Missing,
        })
    }
}

pub fn extract_md5_from_packed_info(blob: &[u8]) -> Option<String> {
    const MARKER: &[u8; 4] = &[0x12, 0x22, 0x0A, 0x20];

    // 主路径
    if let Some(pos) = find_subslice(blob, MARKER) {
        let start = pos + MARKER.len();
        if start + 32 <= blob.len() {
            if let Ok(s) = std::str::from_utf8(&blob[start..start + 32]) {
                if s.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(s.to_ascii_lowercase());
                }
            }
        }
    }

    // Fallback：连续 32 字节合法 hex
    if blob.len() >= 32 {
        for start in 0..=blob.len() - 32 {
            let chunk = &blob[start..start + 32];
            if let Ok(s) = std::str::from_utf8(chunk) {
                if s.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(s.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

/// 简单的子串扫描（避免拉 memchr/memmem 依赖；blob 通常 < 1KB）
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 仅 schema lookup（不去找本地 .dat）。
/// 用于 `wx attachments` 列表时填 `md5` 字段——文件可能根本不在本地。
#[derive(Debug, Clone)]
pub struct AttachmentMetadata {
    pub md5: String,
}

/// 用 `(chat, local_id)` 查 message_resource.db 拿 file md5。
///
/// 调用方传已经解密好的 `message_resource.db` 路径（由 daemon 的 `DBCache` 准备）。
/// 同步函数 — caller 在 `spawn_blocking` 里跑。
pub fn legacy_lookup_md5(
    resource_db_path: &Path,
    chat: &str,
    local_id: i64,
    create_time: i64,
    msg_local_type_lo32: i64,
) -> Result<Option<AttachmentMetadata>> {
    let conn = Connection::open_with_flags(
        resource_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("打开 message_resource.db {:?}", resource_db_path))?;

    // 1) ChatName2Id: user_name -> rowid
    let chat_id: Option<i64> = conn
        .query_row(
            "SELECT rowid FROM ChatName2Id WHERE user_name = ?1",
            [chat],
            |row| row.get(0),
        )
        .ok();
    let Some(chat_id) = chat_id else {
        return Ok(None);
    };

    // 2) MessageResourceInfo:
    //    同 chat 内 local_id 会复用，所以先用 create_time 精确命中；
    //    若资源库里的时间戳跟 message_N.db 不完全对齐，再 fallback 到“同 local_id/type 取最新”
    //    message_local_type 高 32 bit 是版本/会话 flag，低 32 bit 才是真实类型
    let packed_exact: Option<Vec<u8>> = conn
        .query_row(
            "SELECT packed_info FROM MessageResourceInfo
             WHERE chat_id = ?1
               AND message_local_id = ?2
               AND (message_local_type = ?3 OR message_local_type % 4294967296 = ?3)
               AND message_create_time = ?4
             ORDER BY rowid DESC
             LIMIT 1",
            rusqlite::params![chat_id, local_id, msg_local_type_lo32, create_time],
            |row| row.get(0),
        )
        .ok();

    let packed: Option<Vec<u8>> = packed_exact.or_else(|| {
        conn.query_row(
            "SELECT packed_info FROM MessageResourceInfo
             WHERE chat_id = ?1
               AND message_local_id = ?2
               AND (message_local_type = ?3 OR message_local_type % 4294967296 = ?3)
             ORDER BY message_create_time DESC
             LIMIT 1",
            rusqlite::params![chat_id, local_id, msg_local_type_lo32],
            |row| row.get(0),
        )
        .ok()
    });

    let Some(blob) = packed else {
        return Ok(None);
    };
    Ok(extract_md5_from_packed_info(&blob).map(|md5| AttachmentMetadata { md5 }))
}
