//! Read-only planning sources. Source layout and SQL do not cross this boundary.
pub(crate) mod scan;
use super::messages::read::layout;
use crate::business::chat_plan::{
    self as domain, MessageStatistics, PlanChat, ReadFailure, TimeRange,
};
use anyhow::{bail, ensure, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct PlanDatabases {
    pub message: Vec<PathBuf>,
    pub resource: Option<PathBuf>,
    pub media: Vec<PathBuf>,
}

fn safe_table(name: &str) -> bool {
    name.strip_prefix("Msg_").is_some_and(|hash| {
        layout::canonical_table_hash(&format!("Msg_{}", hash.to_ascii_lowercase())).is_some()
    })
}

/// NULL 时间保留为 None；TEXT length 的字符计数语义与 Python SQLite 一致。
pub fn query_message_table_plan_stats(
    conn: &Connection,
    table: &str,
    range: TimeRange,
) -> Result<MessageStatistics> {
    domain::validate_range(range)?;
    ensure!(safe_table(table), "非法消息表名");
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?;
    ensure!(exists, "消息表不存在或不是实体表");
    let sql = format!("SELECT COUNT(*), MIN(create_time), MAX(create_time), COALESCE(SUM(COALESCE(length(message_content),0) + COALESCE(length(compress_content),0) + COALESCE(length(packed_info_data),0)),0) FROM [{table}] WHERE (?1 IS NULL OR create_time >= ?1) AND (?2 IS NULL OR create_time <= ?2)");
    Ok(conn.query_row(&sql, params![range.start, range.end], |r| {
        Ok(MessageStatistics {
            message_count: r.get(0)?,
            first_ts: r.get(1)?,
            last_ts: r.get(2)?,
            message_body_bytes: r.get(3)?,
        })
    })?)
}
fn regular_component(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink(), "拒绝符号链接路径");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(metadata.file_attributes() & 0x400 == 0, "拒绝重解析点路径");
    }
    Ok(())
}

fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(!relative.as_os_str().is_empty(), "库路径不能为空");
    let mut path = root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            bail!("库路径必须是根目录内的相对路径")
        };
        ensure!(!name.to_string_lossy().contains(':'), "拒绝备用数据流路径");
        path.push(name);
        // 缺失文件交由统计层呈现 *_error，不创建数据库。
        if path.try_exists()? {
            regular_component(&path)?;
        }
    }
    if path.try_exists()? {
        ensure!(path.canonicalize()?.starts_with(root), "库路径越界");
        ensure!(path.is_file(), "库路径不是文件");
    }
    Ok(path)
}

#[cfg(test)]
pub(crate) fn open_test_source(path: &Path) -> Result<Connection> {
    open_readonly(path)
}

fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    Ok(conn)
}
const RESOURCE_SQL: &str = "SELECT COALESCE(SUM(COALESCE(d.size,0)),0) FROM ChatName2Id c JOIN MessageResourceInfo i ON i.chat_id=c.rowid LEFT JOIN MessageResourceDetail d ON d.message_id=i.message_id WHERE c.user_name=?1 AND (?2 IS NULL OR i.message_create_time>=?2) AND (?3 IS NULL OR i.message_create_time<=?3)";
const MEDIA_SQL: &str = "SELECT COALESCE(SUM(COALESCE(length(v.voice_data),0)),0) FROM Name2Id n JOIN VoiceInfo v ON v.chat_name_id=n.rowid WHERE n.user_name=?1 AND (?2 IS NULL OR v.create_time>=?2) AND (?3 IS NULL OR v.create_time<=?3)";
fn attachment_values(
    path: &Path,
    sql: &str,
    chats: &[PlanChat],
    range: TimeRange,
) -> Result<Vec<i64>> {
    let conn = open_readonly(path)?;
    let mut statement = conn.prepare(sql)?;
    chats
        .iter()
        .map(|chat| {
            Ok(
                statement
                    .query_row(params![chat.username, range.start, range.end], |r| r.get(0))?,
            )
        })
        .collect()
}

pub struct SqliteSource {
    databases: PlanDatabases,
}
impl SqliteSource {
    /// The host pins the explicit root for the complete read/scan operation.
    pub fn new(root: &Path, databases: &PlanDatabases) -> Result<Self> {
        let resolve = |paths: &[PathBuf]| -> Result<Vec<PathBuf>> {
            let mut seen = BTreeSet::new();
            paths
                .iter()
                .map(|path| {
                    let path = checked_path(root, path)?;
                    ensure!(
                        seen.insert(path.to_string_lossy().to_lowercase()),
                        "重复数据库路径"
                    );
                    Ok(path)
                })
                .collect()
        };
        Ok(Self {
            databases: PlanDatabases {
                message: resolve(&databases.message)?,
                media: resolve(&databases.media)?,
                resource: databases
                    .resource
                    .as_ref()
                    .map(|p| checked_path(root, p))
                    .transpose()?,
            },
        })
    }
}
impl domain::Source for SqliteSource {
    fn message_source_count(&self) -> usize {
        self.databases.message.len()
    }
    fn media_source_count(&self) -> usize {
        self.databases.media.len()
    }
    fn message_contribution(
        &mut self,
        source_index: usize,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> domain::Read<Vec<domain::Read<Option<MessageStatistics>>>> {
        let conn = open_readonly(&self.databases.message[source_index]).map_err(|_| ReadFailure)?;
        Ok(chats
            .iter()
            .map(|chat| {
                let table = layout::table_for_username(&chat.username);
                let exists = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                        [&table],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|_| ReadFailure)?;
                if !exists {
                    return Ok(None);
                }
                query_message_table_plan_stats(&conn, &table, range)
                    .map(Some)
                    .map_err(|_| ReadFailure)
            })
            .collect())
    }
    fn resource_contribution(
        &mut self,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> domain::Read<Option<Vec<i64>>> {
        self.databases
            .resource
            .as_ref()
            .map(|path| attachment_values(path, RESOURCE_SQL, chats, range))
            .transpose()
            .map_err(|_| ReadFailure)
    }
    fn media_contribution(
        &mut self,
        source_index: usize,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> domain::Read<Vec<i64>> {
        attachment_values(&self.databases.media[source_index], MEDIA_SQL, chats, range)
            .map_err(|_| ReadFailure)
    }
}

fn shards(root: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let directory = root.join("message");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(".db"))
        else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        ensure!(entry.file_type()?.is_file(), "数据库分片不是普通文件");
        paths.push(PathBuf::from("message").join(name));
    }
    paths.sort();
    Ok(paths)
}
/// This is the existing cache inventory, not a complete account/source assertion.
pub fn cached_databases(root: &Path) -> Result<PlanDatabases> {
    let fixed = PathBuf::from("message/message_resource.db");
    let resource = if root.join(&fixed).try_exists()? {
        Some(fixed)
    } else {
        let mut paths = shards(root, "message_resource_")?;
        ensure!(
            paths.len() <= 1,
            "当前计划统计需要单一 message_resource 库，不能静默忽略其他分片"
        );
        paths.pop()
    };
    Ok(PlanDatabases {
        message: shards(root, "message_")?,
        resource,
        media: shards(root, "media_")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Partial, Plan};
    fn chats() -> Vec<PlanChat> {
        vec![PlanChat {
            index: 1,
            username: "u".into(),
            chat_name: "u".into(),
            chat_type: "single".into(),
        }]
    }
    #[test]
    fn real_sql_preserves_text_length_null_zero_and_inclusive_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("message.db");
        let conn = Connection::open(&path).unwrap();
        let table = layout::table_for_username("u");
        conn.execute_batch(&format!("CREATE TABLE [{table}](create_time INTEGER, message_content TEXT, compress_content BLOB, packed_info_data BLOB);
            INSERT INTO [{table}] VALUES (0, '中文', X'0102', NULL), (10, NULL, NULL, X'03'), (11, 'outside', NULL, NULL);")).unwrap();
        let stats = query_message_table_plan_stats(
            &conn,
            &table,
            TimeRange {
                start: Some(0),
                end: Some(10),
            },
        )
        .unwrap();
        assert_eq!(
            stats,
            MessageStatistics {
                message_count: 2,
                first_ts: Some(0),
                last_ts: Some(10),
                message_body_bytes: 5
            }
        );
        drop(conn);
        let mut source = SqliteSource::new(
            &root,
            &PlanDatabases {
                message: vec!["message.db".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let plan = Plan::read(
            &mut source,
            &chats(),
            TimeRange {
                start: Some(0),
                end: Some(10),
            },
        )
        .unwrap();
        assert_eq!(plan.rows[0].messages.first_ts, None);
        assert_eq!(plan.rows[0].total_estimated_bytes, 5);
    }
    #[test]
    fn real_resource_and_media_contributions_retain_first_failure_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        Connection::open(root.join("resource.db")).unwrap().execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id VALUES ('u');
            CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_id INTEGER,message_create_time INTEGER); INSERT INTO MessageResourceInfo VALUES (1,7,10);
            CREATE TABLE MessageResourceDetail(message_id INTEGER,size INTEGER); INSERT INTO MessageResourceDetail VALUES (7,8),(7,NULL);").unwrap();
        for name in ["a.db", "later.db"] {
            Connection::open(root.join(name)).unwrap().execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('u'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,voice_data BLOB); INSERT INTO VoiceInfo VALUES (1,10,X'010203');").unwrap();
        }
        let mut source = SqliteSource::new(
            &root,
            &PlanDatabases {
                resource: Some("resource.db".into()),
                media: vec!["a.db".into(), "missing.db".into(), "later.db".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let plan = Plan::read(
            &mut source,
            &chats(),
            TimeRange {
                start: Some(10),
                end: Some(10),
            },
        )
        .unwrap();
        assert_eq!(plan.rows[0].attachment_estimated_bytes, 11);
        assert!(plan.rows[0].statuses.contains(&Partial::MediaReadFailed));
        assert!(!root.join("missing.db").exists());
    }
    #[test]
    fn cached_inventory_preserves_numeric_order_and_resource_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("message")).unwrap();
        for name in [
            "message_2.db",
            "message_10.db",
            "message_old.db",
            "media_1.db",
            "message_resource_0.db",
            "message_resource_1.db",
        ] {
            fs::write(root.join("message").join(name), []).unwrap();
        }
        assert!(cached_databases(root).is_err());
        fs::write(root.join("message/message_resource.db"), []).unwrap();
        let inventory = cached_databases(root).unwrap();
        assert_eq!(
            inventory.message,
            vec![
                PathBuf::from("message/message_10.db"),
                PathBuf::from("message/message_2.db")
            ]
        );
        assert_eq!(
            inventory.resource,
            Some("message/message_resource.db".into())
        );
        assert_eq!(inventory.media, vec![PathBuf::from("message/media_1.db")]);
    }
}
