//! 企业微信离线联系人、会话和消息查询；仅打开调用者指定目录，不发现账号或读取配置。
//! 对齐 export_wxwork_messages.py；计数保留原始行数，消息按三表优先级去重。

use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[path = "query_content.rs"]
mod content;
#[path = "query_export.rs"]
mod export;
pub use export::ExportFormat;

const TABLES: [&str; 3] = ["message_table", "message_small_table", "kf_message_tableV1"];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Contact {
    pub id: i64,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Conversation {
    pub conversation_id: String,
    pub display_name: String,
    pub kind: String,
    pub message_count: u64,
    pub last_time: i64,
    pub last_message_id: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    pub source_table: String,
    pub message_id: i64,
    pub server_id: i64,
    pub sequence: i64,
    pub conversation_id: String,
    pub conversation: String,
    pub conversation_kind: String,
    pub sender_id: i64,
    pub sender: String,
    pub content_type: i64,
    pub type_name: String,
    pub send_time: i64,
    pub time: String,
    pub flag: i64,
    pub content: String,
    pub extra_content: String,
    pub local_extra_content: String,
    pub display_content: String,
    pub is_sent: bool,
}

/// 时间范围使用库中原始时间单位，含起点不含终点；空会话列表表示全部会话。
/// 先按旧版三表优先级去重，再过滤、全局排序、分页；None 表示不限制条数。
#[derive(Debug, Default, Clone)]
pub struct MessageFilter {
    pub conversation_ids: Vec<String>,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub sender_id: Option<i64>,
    pub content_type: Option<i64>,
    pub contains: Option<String>,
    pub offset: usize,
    pub limit: Option<usize>,
}

pub struct OfflineStore {
    directory: PathBuf,
    self_id: Option<i64>,
    message_db: Connection,
    user_db: Option<Connection>,
    session_db: Option<Connection>,
}

fn exists(db: &Connection, table: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}

fn open_readonly(path: &Path, optional: bool) -> Result<Option<Connection>> {
    match fs::symlink_metadata(path) {
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => ensure!(
            result?.file_type().is_file(),
            "Database must be a regular file, not a symlink"
        ),
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match fs::symlink_metadata(Path::new(&sidecar)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => anyhow::bail!("Offline snapshot required: database sidecar exists"),
        }
    }
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN;")?;
    // 开启读事务并检查结构，后续查询保持同一个数据库快照。
    let mut check = db.prepare("PRAGMA integrity_check")?;
    let mut rows = check.query([])?;
    ensure!(
        rows.next()?
            .context("Missing SQLite integrity result")?
            .get::<_, String>(0)?
            == "ok",
        "SQLite integrity_check failed"
    );
    ensure!(rows.next()?.is_none(), "SQLite integrity_check failed");
    drop(rows);
    drop(check);
    Ok(Some(db))
}

fn text(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    Ok(row.get::<_, Option<String>>(index)?.unwrap_or_default())
}
fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<i64> {
    Ok(row.get::<_, Option<i64>>(index)?.unwrap_or_default())
}
fn first(values: &[String]) -> String {
    values
        .iter()
        .find(|v| !v.is_empty())
        .cloned()
        .unwrap_or_default()
}

fn kind(cid: &str) -> &'static str {
    match cid.get(..2) {
        Some("R:") => "群聊",
        Some("S:") => "单聊",
        Some("M:") => "微信联系人",
        Some("O:") => "应用/公众号",
        Some("Y:") => "系统会话",
        _ => "其他",
    }
}

impl OfflineStore {
    /// 目录必须是单一账号的已解密离线快照；不推断本人 ID，None 时不标记“我”。
    pub fn open(directory: &Path, self_id: Option<i64>) -> Result<Self> {
        let directory = fs::canonicalize(directory).context("Offline directory does not exist")?;
        ensure!(directory.is_dir(), "Offline directory must be a directory");
        let message_db =
            open_readonly(&directory.join("message.db"), false)?.context("Missing message.db")?;
        let user_db = open_readonly(&directory.join("user.db"), true)?;
        let session_db = open_readonly(&directory.join("session.db"), true)?;
        let mut found = false;
        for table in TABLES {
            found |= exists(&message_db, table)?;
        }
        ensure!(found, "No supported enterprise message tables");
        Ok(Self {
            directory,
            self_id,
            message_db,
            user_db,
            session_db,
        })
    }

    pub fn contacts(&self) -> Result<Vec<Contact>> {
        Ok(self
            .user_map()?
            .into_iter()
            .map(|(id, display_name)| Contact { id, display_name })
            .collect())
    }

    fn user_map(&self) -> Result<BTreeMap<i64, String>> {
        let mut users = BTreeMap::new();
        let Some(db) = &self.user_db else {
            return Ok(users);
        };
        if exists(db, "user_table")? {
            let mut stmt = db.prepare("SELECT id,name,real_name,account,external_corp_name,external_job FROM user_table ORDER BY id")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let mut name = first(&[text(row, 2)?, text(row, 1)?, text(row, 3)?]);
                let corp = text(row, 4)?;
                if !corp.is_empty() && !name.contains(&corp) {
                    name = if name.is_empty() {
                        corp
                    } else {
                        format!("{name} ({corp})")
                    };
                }
                if !name.is_empty() {
                    users.insert(number(row, 0)?, name);
                }
            }
        }
        if exists(db, "external_user_relation_v3")? {
            let mut stmt = db.prepare("SELECT user_id,remarks,real_remarks,corp_remark FROM external_user_relation_v3 ORDER BY user_id")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name = first(&[text(row, 2)?, text(row, 1)?, text(row, 3)?]);
                if !name.is_empty() {
                    users.insert(number(row, 0)?, name);
                }
            }
        }
        Ok(users)
    }

    fn member_names(&self) -> Result<BTreeMap<(String, i64), String>> {
        let mut names = BTreeMap::new();
        let Some(db) = &self.session_db else {
            return Ok(names);
        };
        if exists(db, "conversation_user_table")? {
            let mut stmt = db
                .prepare("SELECT conversation_id,user_id,nick_name FROM conversation_user_table")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let nickname = text(row, 2)?;
                if !nickname.is_empty() {
                    names.insert((text(row, 0)?, number(row, 1)?), nickname);
                }
            }
        }
        if exists(db, "conversation_member_nickname_table")? && exists(db, "conversation_table")? {
            let mut stmt = db.prepare("SELECT c.id,n.userid,n.nickname FROM conversation_member_nickname_table n JOIN conversation_table c ON c.con_numeric_id=n.room_id")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let nickname = text(row, 2)?;
                if !nickname.is_empty() {
                    names.insert((text(row, 0)?, number(row, 1)?), nickname);
                }
            }
        }
        Ok(names)
    }

    fn conversation_name(&self, cid: &str, users: &BTreeMap<i64, String>) -> String {
        if let Some(tail) = cid.strip_prefix("S:") {
            let ids: Vec<i64> = tail.split('_').filter_map(|s| s.parse().ok()).collect();
            let others: Vec<_> = ids.iter().filter(|id| Some(**id) != self.self_id).collect();
            let selected = if others.is_empty() {
                ids.iter().collect()
            } else {
                others
            };
            for id in selected {
                if let Some(name) = users.get(id) {
                    return name.clone();
                }
            }
        }
        if let Some((_, tail)) = cid.split_once(':') {
            if let Ok(id) = tail.parse::<i64>() {
                if let Some(name) = users.get(&id) {
                    return name.clone();
                }
            }
        }
        cid.to_owned()
    }

    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        let users = self.user_map()?;
        let mut conversations = BTreeMap::new();
        if let Some(db) = &self.session_db {
            if exists(db, "conversation_table")? {
                let mut stmt = db.prepare("SELECT id,name,roomname_remark,last_message_time,last_message_id FROM conversation_table")?;
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let cid = text(row, 0)?;
                    if cid.is_empty() {
                        continue;
                    }
                    let name = first(&[text(row, 2)?, text(row, 1)?]);
                    conversations.insert(
                        cid.clone(),
                        Conversation {
                            display_name: if name.is_empty() {
                                self.conversation_name(&cid, &users)
                            } else {
                                name
                            },
                            kind: kind(&cid).into(),
                            conversation_id: cid,
                            message_count: 0,
                            last_time: number(row, 3)?,
                            last_message_id: number(row, 4)?,
                        },
                    );
                }
            }
        }
        for table in TABLES {
            if !exists(&self.message_db, table)? {
                continue;
            }
            // 表名仅来自固定白名单，用户会话 ID 始终以参数传递。
            let mut stmt = self.message_db.prepare(&format!("SELECT conversation_id,COUNT(*),MAX(send_time) FROM \"{table}\" GROUP BY conversation_id"))?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let cid = text(row, 0)?;
                if cid.is_empty() {
                    continue;
                }
                let conv = conversations
                    .entry(cid.clone())
                    .or_insert_with(|| Conversation {
                        display_name: self.conversation_name(&cid, &users),
                        kind: kind(&cid).into(),
                        conversation_id: cid,
                        message_count: 0,
                        last_time: 0,
                        last_message_id: 0,
                    });
                conv.message_count += row.get::<_, u64>(1)?;
                conv.last_time = conv.last_time.max(number(row, 2)?);
            }
        }
        let mut result: Vec<_> = conversations
            .into_values()
            .filter(|c| c.message_count > 0)
            .collect();
        result.sort_by(|a, b| {
            b.last_time
                .cmp(&a.last_time)
                .then(b.message_count.cmp(&a.message_count))
                .then(a.conversation_id.cmp(&b.conversation_id))
        });
        Ok(result)
    }

    pub fn messages(&self, filter: &MessageFilter) -> Result<Vec<Message>> {
        if let (Some(start), Some(end)) = (filter.start_time, filter.end_time) {
            ensure!(start <= end, "Invalid time range");
        }
        let users = self.user_map()?;
        let members = self.member_names()?;
        let conversations: BTreeMap<_, _> = self
            .conversations()?
            .into_iter()
            .map(|c| (c.conversation_id.clone(), c))
            .collect();
        let mut seen = HashSet::new();
        let mut messages = Vec::new();
        let selected = serde_json::to_string(&filter.conversation_ids)?;
        for table in TABLES {
            if !exists(&self.message_db, table)? {
                continue;
            }
            let mut stmt = self.message_db.prepare(&format!("SELECT message_id,server_id,sequence,sender_id,conversation_id,content_type,send_time,flag,content,extra_content,local_extra_content FROM \"{table}\" WHERE (?1 OR conversation_id IN (SELECT value FROM json_each(?2))) ORDER BY send_time,sequence,message_id"))?;
            let mut rows = stmt.query(params![filter.conversation_ids.is_empty(), selected])?;
            while let Some(row) = rows.next()? {
                let cid = text(row, 4)?;
                let (message_id, server_id, sequence) =
                    (number(row, 0)?, number(row, 1)?, number(row, 2)?);
                if !seen.insert((cid.clone(), message_id, server_id, sequence)) {
                    continue;
                }
                let sender_id = number(row, 3)?;
                let content_type = number(row, 5)?;
                let send_time = number(row, 6)?;
                if filter.start_time.is_some_and(|v| send_time < v)
                    || filter.end_time.is_some_and(|v| send_time >= v)
                    || filter.sender_id.is_some_and(|v| sender_id != v)
                    || filter.content_type.is_some_and(|v| content_type != v)
                {
                    continue;
                }
                let is_sent = self.self_id == Some(sender_id);
                let sender = if is_sent {
                    "我".into()
                } else {
                    members
                        .get(&(cid.clone(), sender_id))
                        .or_else(|| users.get(&sender_id))
                        .cloned()
                        .unwrap_or_else(|| {
                            if sender_id == 0 {
                                "系统".into()
                            } else {
                                sender_id.to_string()
                            }
                        })
                };
                let content = content::decode_value(row.get_ref(8)?)?;
                let extra_content = content::decode_value(row.get_ref(9)?)?;
                let local_extra_content = content::decode_value(row.get_ref(10)?)?;
                let type_name = content::type_name(content_type);
                let mut display_content = first(&[
                    content.clone(),
                    extra_content.clone(),
                    local_extra_content.clone(),
                ]);
                if display_content.is_empty() {
                    display_content = format!("[{type_name}]");
                }
                if filter
                    .contains
                    .as_ref()
                    .is_some_and(|v| !display_content.contains(v))
                {
                    continue;
                }
                let conv = conversations.get(&cid);
                messages.push(Message {
                    source_table: table.into(),
                    message_id,
                    server_id,
                    sequence,
                    conversation: conv.map_or_else(|| cid.clone(), |c| c.display_name.clone()),
                    conversation_kind: kind(&cid).into(),
                    conversation_id: cid,
                    sender_id,
                    sender,
                    content_type,
                    type_name,
                    send_time,
                    time: content::format_time(send_time),
                    flag: number(row, 7)?,
                    content,
                    extra_content,
                    local_extra_content,
                    display_content,
                    is_sent,
                });
            }
        }
        messages.sort_by(|a, b| {
            (
                a.send_time,
                a.sequence,
                a.message_id,
                &a.conversation_id,
                &a.source_table,
            )
                .cmp(&(
                    b.send_time,
                    b.sequence,
                    b.message_id,
                    &b.conversation_id,
                    &b.source_table,
                ))
        });
        Ok(messages
            .into_iter()
            .skip(filter.offset)
            .take(filter.limit.unwrap_or(usize::MAX))
            .collect())
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
