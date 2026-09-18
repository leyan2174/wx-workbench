use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::cache::{CacheMode, DbCache};
use super::meta::{derive_status, discover_unknown_shards, Meta};

mod chat_identity;
#[cfg(test)]
#[path = "query/chat_identity/tests.rs"]
mod chat_identity_tests;
mod contact_rows;
#[cfg(test)]
#[path = "query/contacts_source_tests.rs"]
mod contacts_source_tests;
mod decode;
pub use decode::{q_decode, DecodeKind};
mod export;
pub use export::{q_export_chat, q_export_chat_list, q_export_username};
mod export_directory;
pub use export_directory::{q_export_directory_by_username, q_export_directory_catalog};
#[cfg(test)]
#[path = "../../tests/fixtures/encrypted_cache.rs"]
pub(crate) mod encrypted_cache;
mod export_delta;
#[cfg(test)]
mod favorites_source_tests;
#[cfg(test)]
mod message_read_tests;
#[cfg(test)]
use message_read_tests::{query_messages, search_in_table};
#[cfg(test)]
mod message_source_tests;
pub use export_delta::q_export_delta_username;
pub(super) mod mcp_attachments;
pub(super) mod mcp_contacts;
pub(super) mod mcp_image;
mod mcp_refer;
pub(super) mod mcp_voice;
mod message_read;
mod strict_message;
pub use mcp_refer::q_decode_refer;

const CONTACT_DB_KEY: &str = crate::adapters::wechat::messages::sources::contacts().cache_key();

pub async fn q_resolve_chat(db: &DbCache, names: &Names, chat: &str) -> Result<String> {
    if chat_identity::exact(db, names, chat).await? {
        return Ok(chat.to_owned());
    }
    Ok(mcp_voice::resolve_exact_chat(chat, &names.map)?)
}

/// 判定会话类型。返回值固定为 `group` / `official_account` / `folded` / `private` 之一。
///
/// 判据次序：
/// 1. `@chatroom` / 折叠入口特殊 username
/// 2. `contact.verify_flag` 非 0 —— 覆盖所有被微信官方打了认证标的账号，
///    包括 username 为 `wxid_*` 但实为公众号的情况（如"人物"），
///    以及品牌服务号 `cmb4008205555`、系统号 `qqsafe` / `mphelper` 等
/// 3. username 前缀兜底（`gh_*` / `biz_*` / `@*` 等）—— 在 contact 表未加载或没记录时
///    仍能给出正确结果
pub fn chat_type_of(username: &str, names: &Names) -> &'static str {
    use crate::business::contacts::ContactKind;
    match crate::adapters::wechat::contacts::kind(username, names.is_verified(username)) {
        ContactKind::Group => "group",
        ContactKind::Folded => "folded",
        ContactKind::Official => "official_account",
        ContactKind::Person => "private",
    }
}

/// 联系人名称缓存
#[derive(Clone)]
pub struct Names {
    /// username -> display_name
    pub map: HashMap<String, String>,
    /// 消息 DB 的相对路径列表（message/message_N.db）
    pub msg_db_keys: Vec<String>,
    /// 公众号推送 DB 的相对路径列表（message/biz_message_N.db）
    pub biz_msg_db_keys: Vec<String>,
    /// username -> contact.verify_flag（0=真人，非 0 通常为公众号/服务号/认证账号）
    pub verify_flags: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
struct MessageShard {
    rel_key: String,
    path: std::path::PathBuf,
    table: String,
    max_ts: i64,
    cache_mode: CacheMode,
}

impl Names {
    pub fn display(&self, username: &str) -> String {
        self.map
            .get(username)
            .cloned()
            .unwrap_or_else(|| username.to_string())
    }

    /// 是否被微信官方标了认证/服务号 flag。未在 contact 表中的 username 返回 false。
    pub fn is_verified(&self, username: &str) -> bool {
        self.verify_flags.get(username).copied().unwrap_or(0) != 0
    }
}

fn current_unknown_shards(db: &DbCache, names: &Names) -> Vec<String> {
    discover_unknown_shards(db.db_dir(), &names.msg_db_keys)
}

fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> Result<()> {
    let unknown = super::meta::discover_unknown_shards_checked(db.db_dir(), &names.msg_db_keys)?;
    anyhow::ensure!(
        unknown.is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}

/// 调试路径只受 debug_source 控制，普通元数据开关不得泄露本地路径。
#[derive(Clone, Copy, Default)]
pub struct MetaOptions {
    pub with_meta: bool,
    pub debug_source: bool,
}

/// 时间范围按闭区间处理；未指定类型时保留全部消息类型。
#[derive(Clone, Copy, Default)]
pub struct MessageFilter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub msg_type: Option<i64>,
}

#[derive(Clone, Copy)]
pub struct MessagePage {
    pub limit: usize,
    pub offset: usize,
}

/// 历史查询的全局分页和排序选项，校验由统一选择器完成。
pub struct HistoryQuery<'a> {
    pub page: MessagePage,
    pub filter: MessageFilter,
    pub meta: MetaOptions,
    pub msg_types: Option<&'a [i64]>,
    pub oldest_first: bool,
}

pub struct AttachmentQuery {
    pub kinds: Option<Vec<String>>,
    pub page: MessagePage,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub meta: MetaOptions,
}

#[derive(Clone, Copy)]
#[cfg(test)]
struct MessageView<'a> {
    username: &'a str,
    names: &'a HashMap<String, String>,
    group_nicknames: &'a HashMap<String, String>,
}

impl MetaOptions {
    fn with_details(self) -> bool {
        self.with_meta || self.debug_source
    }
}

fn meta_for_shards(
    scanned: usize,
    shards: &[MessageShard],
    shard_hits: usize,
    unknown_shards: Vec<String>,
    session_last_timestamp: Option<i64>,
    windowed: bool,
    options: MetaOptions,
) -> Meta {
    let latest = shards.first();
    let chat_latest_timestamp = latest.map(|s| s.max_ts);
    Meta {
        chat_latest_timestamp,
        chat_latest_db: latest.map(|s| s.rel_key.clone()),
        session_last_timestamp,
        shards_scanned: scanned,
        shards_hit: shard_hits,
        unknown_shards: unknown_shards.clone(),
        status: derive_status(
            chat_latest_timestamp,
            session_last_timestamp,
            &unknown_shards,
            windowed,
        ),
        per_shard_latest: if options.with_details() {
            Some(
                shards
                    .iter()
                    .map(|s| (s.rel_key.clone(), s.max_ts))
                    .collect(),
            )
        } else {
            None
        },
        cache_mode_per_shard: if options.with_details() {
            Some(
                shards
                    .iter()
                    .map(|s| (s.rel_key.clone(), s.cache_mode.as_str().to_string()))
                    .collect(),
            )
        } else {
            None
        },
        shard_paths: if options.debug_source {
            Some(
                shards
                    .iter()
                    .map(|s| (s.rel_key.clone(), s.path.to_string_lossy().into_owned()))
                    .collect(),
            )
        } else {
            None
        },
    }
}

fn meta_for_global_query(
    scanned: usize,
    hit: usize,
    unknown_shards: Vec<String>,
    windowed: bool,
    options: MetaOptions,
    cache_modes: Option<HashMap<String, String>>,
    shard_paths: Option<HashMap<String, String>>,
) -> Meta {
    Meta {
        chat_latest_timestamp: None,
        chat_latest_db: None,
        session_last_timestamp: None,
        shards_scanned: scanned,
        shards_hit: hit,
        unknown_shards: unknown_shards.clone(),
        status: derive_status(None, None, &unknown_shards, windowed),
        per_shard_latest: if options.with_details() {
            Some(HashMap::new())
        } else {
            None
        },
        cache_mode_per_shard: if options.with_details() {
            cache_modes
        } else {
            None
        },
        shard_paths: if options.debug_source {
            shard_paths
        } else {
            None
        },
    }
}

async fn session_last_timestamp(db: &DbCache, username: &str) -> Option<i64> {
    let path = match db
        .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
        .await
    {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(e) => {
            eprintln!(
                "[freshness] skip session_last_timestamp {}: {}",
                username, e
            );
            return None;
        }
    };

    let username = username.to_string();
    let username_for_query = username.clone();
    match tokio::task::spawn_blocking(move || -> Result<Option<i64>> {
        crate::adapters::wechat::messages::sessions::last_timestamp(&path, &username_for_query)
    })
    .await
    {
        Ok(Ok(ts)) => ts,
        Ok(Err(e)) => {
            eprintln!(
                "[freshness] skip session_last_timestamp {}: {}",
                username, e
            );
            None
        }
        Err(e) => {
            eprintln!(
                "[freshness] task error session_last_timestamp {}: {}",
                username, e
            );
            None
        }
    }
}

/// 加载联系人缓存（从 contact/contact.db）
pub async fn load_names(db: &DbCache) -> Result<Names> {
    use crate::business::contacts::ContactSource;
    let path = db
        .get(CONTACT_DB_KEY)
        .await?
        .context("contact database unavailable")?;
    let directory = tokio::task::spawn_blocking(move || {
        crate::adapters::wechat::contacts::SqliteContacts::new(path).contacts()
    })
    .await??;
    anyhow::ensure!(
        directory.capabilities.classification,
        "unsupported contact classification"
    );
    let mut map = HashMap::new();
    let mut verify_flags = HashMap::new();
    for contact in directory.contacts {
        verify_flags.insert(
            contact.id.0.clone(),
            i64::from(contact.verified.unwrap_or(false)),
        );
        map.insert(contact.id.0.clone(), contact.display().to_owned());
    }
    Ok(Names {
        map,
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags,
    })
}

/// 加载联系人失败时丢弃损坏缓存并重试。
pub async fn load_names_with_retry(
    db: &DbCache,
    attempts: usize,
    delay: Duration,
) -> Result<Names> {
    let attempts = attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=attempts {
        match load_names(db).await {
            Ok(names) => return Ok(names),
            Err(error) => {
                eprintln!(
                    "[daemon] 加载联系人失败 ({}/{}): {}",
                    attempt, attempts, error
                );
                last_error = Some(error);
            }
        }
        if attempt < attempts {
            db.invalidate(CONTACT_DB_KEY).await;
            tokio::time::sleep(delay).await;
        }
    }

    let error = last_error.expect("至少执行一次联系人加载");
    Err(anyhow::anyhow!(
        "重试 {} 次后仍无法加载联系人: {}",
        attempts,
        error
    ))
}

/// 会话表的固定投影。可空字段沿用兼容默认值，用户名缺失则报错。
type SessionRow = crate::adapters::wechat::messages::sessions::Record;

async fn session_view(
    db: &DbCache,
    names: &Names,
    record: SessionRow,
    nickname_cache: &mut HashMap<String, HashMap<String, String>>,
) -> Value {
    let row = record.session;
    let chat_type = chat_type_of(&row.username, names);
    let is_group = chat_type == "group";
    let last_sender = if is_group {
        if let Some(sender) = &row.sender {
            if !nickname_cache.contains_key(&row.username) {
                nickname_cache.insert(
                    row.username.clone(),
                    load_group_nicknames(db, &row.username)
                        .await
                        .unwrap_or_default(),
                );
            }
            sender_display(
                sender,
                row.sender_display_hint.as_deref().unwrap_or(""),
                &names.map,
                &nickname_cache[&row.username],
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    json!({
        "chat": names.display(&row.username), "username": row.username,
        "is_group": is_group, "chat_type": chat_type, "unread": row.unread,
        "last_msg_type": record.type_label, "last_sender": last_sender,
        "summary": row.summary, "timestamp": row.timestamp,
        "time": fmt_time(row.timestamp, "%m-%d %H:%M"),
    })
}

fn session_meta(db: &DbCache, names: &Names, results: &[Value], with_details: bool) -> Meta {
    let latest_ts = results
        .first()
        .and_then(|v| v.get("timestamp"))
        .and_then(Value::as_i64);
    let unknown_shards = current_unknown_shards(db, names);
    Meta {
        chat_latest_timestamp: latest_ts,
        chat_latest_db: latest_ts.map(|_| "session/session.db".to_string()),
        session_last_timestamp: None,
        shards_scanned: 0,
        shards_hit: 0,
        status: derive_status(latest_ts, None, &unknown_shards, false),
        unknown_shards,
        per_shard_latest: with_details.then(HashMap::new),
        cache_mode_per_shard: None,
        shard_paths: None,
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn ordinary_metadata_never_includes_debug_paths() {
        for with_meta in [false, true] {
            for debug_source in [false, true] {
                let meta = meta_for_global_query(
                    2,
                    1,
                    Vec::new(),
                    true,
                    MetaOptions {
                        with_meta,
                        debug_source,
                    },
                    Some(HashMap::from([("message_0.db".into(), "cache_hit".into())])),
                    Some(HashMap::from([(
                        "message_0.db".into(),
                        "private-path".into(),
                    )])),
                );
                assert_eq!(meta.per_shard_latest.is_some(), with_meta || debug_source);
                assert_eq!(
                    meta.cache_mode_per_shard.is_some(),
                    with_meta || debug_source
                );
                assert_eq!(meta.shard_paths.is_some(), debug_source);
            }
        }
    }

    #[test]
    fn nullable_session_fields_keep_defaults() {
        let record =
            session_fixture("SELECT 'wxid_demo', NULL, NULL, NULL, NULL, NULL, NULL").unwrap();
        assert_eq!(record.type_label, fmt_type(0));
        let row = record.session;
        assert_eq!(row.username, "wxid_demo");
        assert_eq!((row.unread, row.timestamp), (0, 0));
        assert_eq!(row.last_kind, crate::business::messages::Kind::Unknown);
        assert!(row.summary.is_empty());
        assert!(row.sender.is_none());
        assert!(row.sender_display_hint.is_none());
    }

    #[test]
    fn missing_username_is_not_silently_discarded() {
        assert!(session_fixture("SELECT NULL, 0, '', 0, 0, '', ''").is_err());
    }

    fn session_fixture(select: &str) -> Result<SessionRow> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("session.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(&format!("CREATE TABLE SessionTable(username, unread_count, summary, last_timestamp, last_msg_type, last_msg_sender, last_sender_display_name); INSERT INTO SessionTable {select}"))?;
        drop(conn);
        crate::adapters::wechat::messages::sessions::read(&path, &HashMap::new())?
            .pop()
            .context("missing synthetic session")
    }

    #[tokio::test]
    async fn session_view_preserves_group_sender_and_metadata_fields() {
        let root = tempfile::tempdir().unwrap();
        let db = DbCache::with_dirs(
            root.path().join("source"),
            root.path().join("cache"),
            root.path().join("mtimes.json"),
            HashMap::new(),
        )
        .await
        .unwrap();
        let names = Names {
            map: HashMap::from([
                ("demo@chatroom".into(), "示例群".into()),
                ("wxid_demo".into(), "通讯录名称".into()),
            ]),
            msg_db_keys: Vec::new(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        };
        let mut nicknames = HashMap::from([(
            "demo@chatroom".into(),
            HashMap::from([("wxid_demo".into(), "群昵称".into())]),
        )]);
        let row = session_fixture(
            "SELECT 'demo@chatroom', 3, 'wxid_demo:\nhello', 1700000000, 1, 'wxid_demo', ''",
        )
        .unwrap();
        let value = session_view(&db, &names, row, &mut nicknames).await;
        assert_eq!(value["chat"], "示例群");
        assert_eq!(value["username"], "demo@chatroom");
        assert_eq!(value["is_group"], true);
        assert_eq!(value["chat_type"], "group");
        assert_eq!(value["unread"], 3);
        assert_eq!(value["last_sender"], "群昵称");
        assert_eq!(value["summary"], "hello");
        assert_eq!(value["timestamp"], 1_700_000_000);
        assert_eq!(value["last_msg_type"], fmt_type(1));
        assert_eq!(value["time"], fmt_time(1_700_000_000, "%m-%d %H:%M"));
        assert_eq!(value.as_object().unwrap().len(), 10);
        let rows = [value];
        let meta = session_meta(&db, &names, &rows, true);
        assert_eq!(meta.chat_latest_timestamp, Some(1_700_000_000));
        assert_eq!(meta.chat_latest_db.as_deref(), Some("session/session.db"));
        assert_eq!(meta.per_shard_latest, Some(HashMap::new()));
        let empty = session_meta(&db, &names, &[], false);
        assert_eq!(empty.chat_latest_timestamp, None);
        assert_eq!(empty.chat_latest_db, None);
        assert_eq!(empty.per_shard_latest, None);
    }
}

/// 查询最近会话列表
pub async fn q_sessions(
    db: &DbCache,
    names: &Names,
    limit: usize,
    with_meta: bool,
    debug_source: bool,
) -> Result<Value> {
    message_read::sessions(
        db,
        names,
        crate::business::sessions::Query {
            limit,
            unread_only: false,
            kinds: Vec::new(),
        },
        with_meta || debug_source,
    )
    .await
}

/// 查询聊天记录
pub async fn q_history(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: HistoryQuery<'_>,
) -> Result<Value> {
    message_read::history(db, names, chat, options).await
}

/// 搜索消息
pub async fn q_search(
    db: &DbCache,
    names: &Names,
    keyword: &str,
    chats: Option<Vec<String>>,
    limit: usize,
    filter: MessageFilter,
    meta: MetaOptions,
) -> Result<Value> {
    message_read::search(db, names, keyword, chats, limit, filter, meta).await
}

/// 查询联系人
///
/// 普通协议投影只列真人；筛选与分页由联系人业务用例执行，分类由微信适配器解释。
pub async fn q_contacts(names: &Names, query: Option<&str>, limit: usize) -> Result<Value> {
    use crate::business::contacts::{self, ContactQuery};
    anyhow::ensure!(
        !names.map.is_empty(),
        "联系人缓存不可用，请执行 `wx daemon reload` 后重试"
    );
    let source =
        crate::adapters::wechat::contacts::cached_directory(&names.map, &names.verify_flags);
    let page = contacts::list(
        &source,
        ContactQuery {
            text: query,
            offset: 0,
            limit,
        },
    )?;
    contact_rows::project_page(page)
}

#[cfg(test)]
mod contact_tests {
    use super::*;
    #[tokio::test]
    async fn empty_contact_cache_is_an_error() {
        let names = Names {
            map: HashMap::new(),
            msg_db_keys: Vec::new(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        };
        let error = q_contacts(&names, None, 20).await.unwrap_err();
        assert!(error.to_string().contains("wx daemon reload"));
    }
}

// Internal query helpers for domain adapters and response projections.

async fn find_msg_shards(
    db: &DbCache,
    names: &Names,
    username: &str,
) -> Result<(Vec<MessageShard>, usize)> {
    message_read::find_shards(db, names, username).await
}

async fn load_group_nicknames(
    db: &DbCache,
    chat_username: &str,
) -> Result<HashMap<String, String>> {
    if !chat_username.contains("@chatroom") {
        return Ok(HashMap::new());
    }
    let Some(contact_p) = db.get(CONTACT_DB_KEY).await? else {
        return Ok(HashMap::new());
    };
    let chat = chat_username.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = Connection::open(&contact_p)?;
        Ok::<_, anyhow::Error>(load_group_nickname_map_from_conn(&conn, &chat, None))
    })
    .await?
}

async fn load_group_nickname_maps(
    db: &DbCache,
    chat_usernames: HashSet<String>,
) -> Result<HashMap<String, HashMap<String, String>>> {
    if chat_usernames.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(contact_p) = db.get(CONTACT_DB_KEY).await? else {
        return Ok(HashMap::new());
    };
    tokio::task::spawn_blocking(move || {
        let conn = Connection::open(&contact_p)?;
        let mut out = HashMap::new();
        for chat in chat_usernames {
            let nicknames = load_group_nickname_map_from_conn(&conn, &chat, None);
            if !nicknames.is_empty() {
                out.insert(chat, nicknames);
            }
        }
        Ok::<_, anyhow::Error>(out)
    })
    .await?
}

use crate::adapters::wechat::contacts::nicknames::load_group_nickname_map_from_conn;
#[cfg(test)]
use crate::adapters::wechat::contacts::nicknames::{decode_proto_varint, parse_group_nickname_map};

fn sender_display(
    username: &str,
    fallback_sender_name: &str,
    names: &HashMap<String, String>,
    group_nicknames: &HashMap<String, String>,
) -> String {
    if username.is_empty() {
        return String::new();
    }
    group_nicknames
        .get(username)
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| names.get(username).cloned())
        .or_else(|| {
            if fallback_sender_name.is_empty() {
                None
            } else {
                Some(fallback_sender_name.to_string())
            }
        })
        .unwrap_or_else(|| username.to_string())
}

fn group_top_senders(
    sender_counts: &HashMap<String, i64>,
    names: &HashMap<String, String>,
    group_nicknames: &HashMap<String, String>,
    limit: usize,
) -> Vec<Value> {
    let mut top_senders: Vec<Value> = sender_counts
        .iter()
        .map(|(username, count)| {
            let mut row = json!({
                "sender": sender_display(username, "", names, group_nicknames),
                "count": count,
            });
            add_sender_identity(&mut row, true, username, names, group_nicknames);
            row
        })
        .collect();
    top_senders.sort_by(|a, b| {
        b["count"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["count"].as_i64().unwrap_or(0))
            .then_with(|| {
                a["sender"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["sender"].as_str().unwrap_or(""))
            })
    });
    top_senders.truncate(limit);
    top_senders
}

fn sender_username(
    real_sender_id: i64,
    content: &str,
    is_group: bool,
    chat_username: &str,
    id2u: &HashMap<i64, String>,
) -> String {
    let sender_uname = id2u.get(&real_sender_id).cloned().unwrap_or_default();
    if !is_group {
        if !sender_uname.is_empty() && sender_uname != chat_username {
            return sender_uname;
        }
        return String::new();
    }
    if !sender_uname.is_empty() && sender_uname != chat_username {
        return sender_uname;
    }
    crate::message::split_group_content(content).0.to_owned()
}

fn add_sender_identity(
    row: &mut Value,
    is_group: bool,
    username: &str,
    names: &HashMap<String, String>,
    group_nicknames: &HashMap<String, String>,
) {
    if !is_group || username.is_empty() {
        return;
    }
    row["sender_username"] = Value::String(username.to_string());
    row["sender_contact_display"] = Value::String(
        names
            .get(username)
            .cloned()
            .unwrap_or_else(|| username.to_string()),
    );
    row["sender_group_nickname"] =
        Value::String(group_nicknames.get(username).cloned().unwrap_or_default());
}

fn sender_label(
    real_sender_id: i64,
    content: &str,
    is_group: bool,
    chat_username: &str,
    id2u: &HashMap<i64, String>,
    names: &HashMap<String, String>,
    group_nicknames: &HashMap<String, String>,
) -> String {
    let sender_uname = id2u.get(&real_sender_id).cloned().unwrap_or_default();
    if is_group {
        if !sender_uname.is_empty() && sender_uname != chat_username {
            return sender_display(&sender_uname, "", names, group_nicknames);
        }
        let raw = crate::message::split_group_content(content).0;
        if !raw.is_empty() {
            return sender_display(raw, "", names, group_nicknames);
        }
        return String::new();
    }
    if !sender_uname.is_empty() && sender_uname != chat_username {
        return names.get(&sender_uname).cloned().unwrap_or(sender_uname);
    }
    String::new()
}

/// 读取消息内容列（兼容 TEXT 和 BLOB 两种存储类型）
///
/// SQLite 中 message_content 在未压缩时为 TEXT，zstd 压缩后为 BLOB。
/// rusqlite 的 Vec<u8> FromSql 只接受 BLOB，读 TEXT 会静默返回空。
fn strip_group_prefix(s: &str) -> String {
    crate::message::split_group_content(s).1.to_owned()
}

pub(crate) use crate::adapters::wechat::messages::display::*;

#[cfg(test)]
mod summary_regression_tests {
    use super::fmt_content;

    #[test]
    fn group_identity_uses_compact_prefix_without_overriding_name2id() {
        let empty = std::collections::HashMap::new();
        let names = std::collections::HashMap::from([("wxid_demo".into(), "示例".into())]);
        let content = "wxid_demo:<msg/>";
        assert_eq!(
            super::sender_username(1, content, true, "room@chatroom", &empty),
            "wxid_demo"
        );
        assert_eq!(
            super::sender_label(
                1,
                content,
                true,
                "room@chatroom",
                &empty,
                &names,
                &std::collections::HashMap::new()
            ),
            "示例"
        );
        let ids = std::collections::HashMap::from([(1, "wxid_authoritative".into())]);
        assert_eq!(
            super::sender_username(1, content, true, "room@chatroom", &ids),
            "wxid_authoritative"
        );
    }

    #[test]
    fn search_rejects_corrupt_rows_instead_of_silently_omitting_them() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE Msg_test(local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content); INSERT INTO Msg_test VALUES ('invalid',1,100,1,'needle',0);").unwrap();
        let empty = std::collections::HashMap::new();
        assert!(super::search_in_table(
            &conn,
            "Msg_test",
            super::MessageView {
                username: "demo",
                names: &empty,
                group_nicknames: &empty
            },
            "needle",
            super::MessageFilter::default(),
            10
        )
        .is_err());
    }

    #[test]
    fn protobuf_varint_rejects_overflow_and_truncation() {
        let mut maximum = vec![0xff; 9];
        maximum.push(1);
        assert_eq!(
            super::decode_proto_varint(&maximum, 0),
            Some((u64::MAX, 10))
        );
        maximum[9] = 2;
        assert_eq!(super::decode_proto_varint(&maximum, 0), None);
        assert_eq!(super::decode_proto_varint(&[0x80; 10], 0), None);
        assert_eq!(super::decode_proto_varint(&[0x80], 0), None);
        assert_eq!(super::decode_proto_varint(&[], usize::MAX), None);
        assert_eq!(super::decode_proto_varint(&[0, 0xac, 2], 1), Some((300, 3)));
    }

    #[test]
    fn summaries_handle_group_prefix_and_flagged_types() {
        assert_eq!(
            fmt_content(
                7,
                (1_i64 << 32) | 50,
                "wxid_demo:\n<voip><msg>Call not answered</msg></voip>",
                true
            ),
            "[通话] 未接听"
        );
        assert_eq!(
            fmt_content(7, 50, "<voip><msg>Duration: 01:23</msg></voip>", false),
            "[通话] 通话时长 01:23"
        );
        assert_eq!(fmt_content(7, 50, "broken", false), "[通话]");
        assert_eq!(
            fmt_content(
                7,
                (1_i64 << 32) | 34,
                "wxid_demo:\n<msg><voicemsg voicelength='3300'/></msg>",
                true
            ),
            "[语音 3.3s]"
        );
        assert_eq!(
            fmt_content(
                7,
                42,
                "wxid_demo:\n<msg nickname='示例' antispamticket='synthetic'/>",
                true
            ),
            "[名片] 示例"
        );
        assert_eq!(
            fmt_content(
                7,
                48,
                "<msg><location poiname='[位置]' label='示例路'/></msg>",
                false
            ),
            "[位置] 示例路"
        );
        assert_eq!(
            fmt_content(7, 42, "<msg antispamticket='synthetic'>", false),
            "[名片]"
        );
        assert_eq!(fmt_content(7, 48, "broken", false), "[位置]");
        assert_eq!(fmt_content(7, 1, "普通文字", false), "普通文字");
    }
}

#[cfg(test)]
use crate::adapters::wechat::favorites::extract_url as extract_favorite_url;

#[cfg(test)]
mod appmsg_tests {
    use super::*;

    #[test]
    fn parse_forwarded_chat_record_expands_record_items() {
        let xml = r#"
<msg>
  <appmsg appid="" sdkver="0">
    <title>群聊的聊天记录</title>
    <des>张三: 早上好
李四: 收到</des>
    <type>19</type>
    <recorditem>&lt;recordinfo&gt;&lt;datalist count="2"&gt;&lt;dataitem datatype="1"&gt;&lt;sourcename&gt;张三&lt;/sourcename&gt;&lt;sourcetime&gt;1710000000&lt;/sourcetime&gt;&lt;datadesc&gt;早上好 &amp;amp; coffee&lt;/datadesc&gt;&lt;/dataitem&gt;&lt;dataitem datatype="2"&gt;&lt;sourcename&gt;李四&lt;/sourcename&gt;&lt;sourcetime&gt;1710000060&lt;/sourcetime&gt;&lt;datafmt&gt;图片&lt;/datafmt&gt;&lt;datadesc&gt;[图片]&lt;/datadesc&gt;&lt;/dataitem&gt;&lt;/datalist&gt;&lt;/recordinfo&gt;</recorditem>
  </appmsg>
</msg>
        "#;

        assert_eq!(
            parse_appmsg(xml).as_deref(),
            Some(
                "[合并聊天记录] 群聊的聊天记录 (2条)\n  - 张三: 早上好 & coffee\n  - 李四: [图片]"
            )
        );
    }

    #[test]
    fn parse_file_appmsg_includes_attachment_metadata() {
        let xml = r#"
<msg>
  <appmsg appid="" sdkver="0">
    <title>report.pdf</title>
    <type>6</type>
    <appattach>
      <totallen>1536</totallen>
      <fileext>pdf</fileext>
    </appattach>
    <md5>abcdef123456</md5>
  </appmsg>
</msg>
        "#;

        assert_eq!(
            parse_appmsg(xml).as_deref(),
            Some("[文件] report.pdf (1.5 KB, pdf)")
        );
    }

    #[test]
    fn parse_quote_appmsg_reads_refermsg_content() {
        let xml = r#"
<msg>
  <appmsg appid="" sdkver="0">
    <title>合成回复正文</title>
    <type>57</type>
    <content />
    <refermsg>
      <type>1</type>
      <displayname>示例联系人</displayname>
      <content>合成引用内容包含 needle 关键词</content>
    </refermsg>
  </appmsg>
</msg>
        "#;

        assert_eq!(
            parse_appmsg(xml).as_deref(),
            Some("[引用] 合成回复正文\n  \u{21b3} 示例联系人: 合成引用内容包含 needle 关键词")
        );
    }

    #[test]
    fn query_messages_filters_appmsg_by_base_type() {
        let path = temp_db_path("query_messages_filters_appmsg_by_base_type");
        {
            let conn = Connection::open(&path).expect("open temp db");
            conn.execute(
                "CREATE TABLE Msg_test (
                    local_id INTEGER,
                    local_type INTEGER,
                    create_time INTEGER,
                    real_sender_id INTEGER,
                    message_content TEXT,
                    WCDB_CT_message_content INTEGER
                )",
                [],
            )
            .expect("create message table");
            conn.execute(
                "INSERT INTO Msg_test VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    1_i64,
                    ((57_i64) << 32) | 49_i64,
                    1775146911_i64,
                    0_i64,
                    r#"<msg><appmsg><title>合成回复正文</title><type>57</type><content /><refermsg><displayname>示例联系人</displayname><content>合成引用内容包含 needle 关键词</content></refermsg></appmsg></msg>"#,
                    0_i64
                ],
            )
            .expect("insert quote message");
        }

        let rows = query_messages(
            &path,
            "Msg_test",
            MessageView {
                username: "wxid_synthetic_peer",
                names: &HashMap::new(),
                group_nicknames: &HashMap::new(),
            },
            MessageFilter {
                msg_type: Some(49),
                ..MessageFilter::default()
            },
            MessagePage {
                limit: 10,
                offset: 0,
            },
        )
        .expect("query messages");

        let _ = std::fs::remove_file(&path);

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["content"].as_str(),
            Some("[引用] 合成回复正文\n  \u{21b3} 示例联系人: 合成引用内容包含 needle 关键词")
        );
    }

    #[test]
    fn query_messages_includes_stable_group_sender_identity() {
        let path = temp_db_path("query_messages_includes_stable_group_sender_identity");
        {
            let conn = Connection::open(&path).expect("open temp db");
            conn.execute(
                "CREATE TABLE Name2Id (
                    user_name TEXT
                )",
                [],
            )
            .expect("create Name2Id table");
            conn.execute(
                "INSERT INTO Name2Id(rowid, user_name) VALUES (?1, ?2)",
                rusqlite::params![42_i64, "wxid_alice"],
            )
            .expect("insert Name2Id row");
            conn.execute(
                "CREATE TABLE Msg_test (
                    local_id INTEGER,
                    local_type INTEGER,
                    create_time INTEGER,
                    real_sender_id INTEGER,
                    message_content TEXT,
                    WCDB_CT_message_content INTEGER
                )",
                [],
            )
            .expect("create message table");
            conn.execute(
                "INSERT INTO Msg_test VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![1_i64, 1_i64, 1775146911_i64, 42_i64, "hello", 0_i64],
            )
            .expect("insert text message");
        }

        let names = HashMap::from([("wxid_alice".to_string(), "Alice Contact".to_string())]);
        let group_nicknames = HashMap::from([("wxid_alice".to_string(), "同名".to_string())]);
        let rows = query_messages(
            &path,
            "Msg_test",
            MessageView {
                username: "123@chatroom",
                names: &names,
                group_nicknames: &group_nicknames,
            },
            MessageFilter::default(),
            MessagePage {
                limit: 10,
                offset: 0,
            },
        )
        .expect("query messages");

        let _ = std::fs::remove_file(&path);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["sender"].as_str(), Some("同名"));
        assert_eq!(rows[0]["sender_username"].as_str(), Some("wxid_alice"));
        assert_eq!(
            rows[0]["sender_contact_display"].as_str(),
            Some("Alice Contact")
        );
        assert_eq!(rows[0]["sender_group_nickname"].as_str(), Some("同名"));
    }

    #[test]
    fn search_in_table_includes_stable_group_sender_identity() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute(
            "CREATE TABLE Name2Id (
                user_name TEXT
            )",
            [],
        )
        .expect("create Name2Id table");
        conn.execute(
            "INSERT INTO Name2Id(rowid, user_name) VALUES (?1, ?2)",
            rusqlite::params![42_i64, "wxid_alice"],
        )
        .expect("insert Name2Id row");
        conn.execute(
            "CREATE TABLE Msg_test (
                local_id INTEGER,
                local_type INTEGER,
                create_time INTEGER,
                real_sender_id INTEGER,
                message_content TEXT,
                WCDB_CT_message_content INTEGER
            )",
            [],
        )
        .expect("create message table");
        conn.execute(
            "INSERT INTO Msg_test VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![1_i64, 1_i64, 1775146911_i64, 42_i64, "needle", 0_i64],
        )
        .expect("insert text message");

        let names = HashMap::from([("wxid_alice".to_string(), "Alice Contact".to_string())]);
        let group_nicknames = HashMap::from([("wxid_alice".to_string(), "同名".to_string())]);
        let rows = search_in_table(
            &conn,
            "Msg_test",
            MessageView {
                username: "123@chatroom",
                names: &names,
                group_nicknames: &group_nicknames,
            },
            "needle",
            MessageFilter::default(),
            10,
        )
        .expect("search messages");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["sender"].as_str(), Some("同名"));
        assert_eq!(rows[0]["sender_username"].as_str(), Some("wxid_alice"));
        assert_eq!(
            rows[0]["sender_contact_display"].as_str(),
            Some("Alice Contact")
        );
        assert_eq!(rows[0]["sender_group_nickname"].as_str(), Some("同名"));
    }

    /// q_attachments 是异步 + 依赖 DbCache，无法直接 unit-test 整条 pipeline。
    /// 这里锁住 attachment row 复用 `add_sender_identity` 后的最终 JSON 形状：
    /// 两个 group nickname 同为 "同名" 的成员，attachment 行可以通过 sender_username 区分。
    #[test]
    fn attachment_row_gets_stable_group_sender_identity_via_helper() {
        let names: HashMap<String, String> = HashMap::from([
            ("wxid_alice".to_string(), "Alice Contact".to_string()),
            ("wxid_bob".to_string(), "Bob Contact".to_string()),
        ]);
        let group_nicknames: HashMap<String, String> = HashMap::from([
            ("wxid_alice".to_string(), "同名".to_string()),
            ("wxid_bob".to_string(), "同名".to_string()),
        ]);

        let mut alice_row = json!({
            "attachment_id": "abc",
            "kind": "image",
            "type": "Image",
            "local_id": 1,
            "timestamp": 1775146911,
            "time": "2026-04-30 12:00",
            "sender": "同名",
        });
        add_sender_identity(&mut alice_row, true, "wxid_alice", &names, &group_nicknames);
        assert_eq!(alice_row["sender"].as_str(), Some("同名"));
        assert_eq!(alice_row["sender_username"].as_str(), Some("wxid_alice"));
        assert_eq!(
            alice_row["sender_contact_display"].as_str(),
            Some("Alice Contact")
        );
        assert_eq!(alice_row["sender_group_nickname"].as_str(), Some("同名"));

        let mut bob_row = json!({
            "attachment_id": "def",
            "kind": "image",
            "type": "Image",
            "local_id": 2,
            "timestamp": 1775146922,
            "time": "2026-04-30 12:00",
            "sender": "同名",
        });
        add_sender_identity(&mut bob_row, true, "wxid_bob", &names, &group_nicknames);
        assert_eq!(bob_row["sender_username"].as_str(), Some("wxid_bob"));
        // 同样 sender_group_nickname 都是 "同名"，但 sender_username 能区分
        assert_ne!(
            alice_row["sender_username"], bob_row["sender_username"],
            "sender_username 必须区分两位同名成员"
        );

        // 非群 chat 不该追加 identity 字段（行为对齐 history/search/new-messages）
        let mut private_row = json!({"attachment_id": "ghi", "sender": ""});
        add_sender_identity(
            &mut private_row,
            false,
            "wxid_alice",
            &names,
            &group_nicknames,
        );
        assert!(private_row.get("sender_username").is_none());
        assert!(private_row.get("sender_contact_display").is_none());
        assert!(private_row.get("sender_group_nickname").is_none());

        // group 但 sender_username 解析为空（非常老的格式、id2u 没命中、content 也没 wxid_xxx:\n 前缀）：
        // 不要伪造空字段，整段 identity 也不追加
        let mut unknown_row = json!({"attachment_id": "jkl", "sender": ""});
        add_sender_identity(&mut unknown_row, true, "", &names, &group_nicknames);
        assert!(unknown_row.get("sender_username").is_none());
        assert!(unknown_row.get("sender_contact_display").is_none());
        assert!(unknown_row.get("sender_group_nickname").is_none());
    }

    #[test]
    fn search_in_table_filters_appmsg_by_base_type() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute(
            "CREATE TABLE Msg_test (
                local_id INTEGER,
                local_type INTEGER,
                create_time INTEGER,
                real_sender_id INTEGER,
                message_content TEXT,
                WCDB_CT_message_content INTEGER
            )",
            [],
        )
        .expect("create message table");
        conn.execute(
            "INSERT INTO Msg_test VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                1_i64,
                ((57_i64) << 32) | 49_i64,
                1775146911_i64,
                0_i64,
                r#"<msg><appmsg><title>合成回复正文</title><type>57</type><content /><refermsg><displayname>示例联系人</displayname><content>合成引用内容包含 needle 关键词</content></refermsg></appmsg></msg>"#,
                0_i64
            ],
        )
        .expect("insert quote message");

        let rows = search_in_table(
            &conn,
            "Msg_test",
            MessageView {
                username: "wxid_synthetic_peer",
                names: &HashMap::new(),
                group_nicknames: &HashMap::new(),
            },
            "needle",
            MessageFilter {
                msg_type: Some(49),
                ..MessageFilter::default()
            },
            10,
        )
        .expect("search messages");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["content"].as_str(),
            Some("[引用] 合成回复正文\n  \u{21b3} 示例联系人: 合成引用内容包含 needle 关键词")
        );
    }

    #[test]
    fn search_in_table_matches_decompressed_formatted_appmsg_content() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute(
            "CREATE TABLE Msg_test (
                local_id INTEGER,
                local_type INTEGER,
                create_time INTEGER,
                real_sender_id INTEGER,
                message_content BLOB,
                WCDB_CT_message_content INTEGER
            )",
            [],
        )
        .expect("create message table");
        let xml = r#"<msg><appmsg><title>合成回复正文</title><type>57</type><content /><refermsg><displayname>示例联系人</displayname><content>合成引用内容包含 needle 关键词</content></refermsg></appmsg></msg>"#;
        let compressed = zstd::encode_all(xml.as_bytes(), 0).expect("compress appmsg xml");
        conn.execute(
            "INSERT INTO Msg_test VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                1_i64,
                ((57_i64) << 32) | 49_i64,
                1775146911_i64,
                0_i64,
                compressed,
                4_i64
            ],
        )
        .expect("insert compressed quote message");

        let rows = search_in_table(
            &conn,
            "Msg_test",
            MessageView {
                username: "wxid_synthetic_peer",
                names: &HashMap::new(),
                group_nicknames: &HashMap::new(),
            },
            "needle",
            MessageFilter {
                msg_type: Some(49),
                ..MessageFilter::default()
            },
            10,
        )
        .expect("search messages");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["content"].as_str(),
            Some("[引用] 合成回复正文\n  \u{21b3} 示例联系人: 合成引用内容包含 needle 关键词")
        );
    }

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "wx-cli-{}-{}-{}.db",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }
}

fn fmt_time(ts: i64, fmt: &str) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format(fmt).to_string())
        .unwrap_or_else(|| ts.to_string())
}

// ─── 新增命令查询函数 ──────────────────────────────────────────────────────────

/// 查询有未读消息的会话
///
/// `filter`：按 chat_type 过滤，None 或空 Vec 等价于 "all"。
/// 可选值：`private` / `group` / `official` / `folded` / `all`。
/// 多选支持在 CLI 层用逗号分隔后传入多个元素。
pub async fn q_unread(
    db: &DbCache,
    names: &Names,
    limit: usize,
    filter: Option<Vec<String>>,
    with_meta: bool,
    debug_source: bool,
) -> Result<Value> {
    use crate::business::contacts::ContactKind;
    let mut kinds = Vec::new();
    for value in filter.unwrap_or_default() {
        match value.trim().to_lowercase().as_str() {
            "" | "all" => {
                kinds.clear();
                break;
            }
            "private" => kinds.push(ContactKind::Person),
            "group" => kinds.push(ContactKind::Group),
            "official" | "official_account" => kinds.push(ContactKind::Official),
            "folded" | "fold" => kinds.push(ContactKind::Folded),
            _ => {}
        }
    }
    message_read::sessions(
        db,
        names,
        crate::business::sessions::Query {
            limit,
            unread_only: true,
            kinds,
        },
        with_meta || debug_source,
    )
    .await
}

/// 查询群成员：优先从 contact.db 的 chatroom_member/chat_room 表获取完整列表，
/// 若表不存在则退化为从消息记录聚合有发言记录的成员
pub async fn q_members(db: &DbCache, names: &Names, chat: &str) -> Result<Value> {
    use crate::adapters::wechat::contacts::SqliteContacts;
    use crate::business::contacts::{self as contacts, Error, MembershipCoverage};
    contacts::validate_query(chat)?;
    let path = db
        .get(CONTACT_DB_KEY)
        .await?
        .context("contact database unavailable")?;
    let mut source = SqliteContacts::new(path);
    source.display_names = names.map.clone();
    let query = chat.to_owned();
    let (mut source, first) = tokio::task::spawn_blocking(move || {
        let result = contacts::members(&source, &query);
        (source, result)
    })
    .await?;
    let (group, membership) = match first {
        Err(Error::Unsupported("group membership")) => {
            ensure_complete_message_inventory(db, names)?;
            for key in &names.msg_db_keys {
                source
                    .message_paths
                    .push(db.get(key).await?.context("message source unavailable")?);
            }
            let query = chat.to_owned();
            tokio::task::spawn_blocking(move || contacts::members(&source, &query)).await??
        }
        other => other?,
    };
    let members: Vec<_> = membership
        .members
        .iter()
        .map(|member| {
            json!({
                "username": member.id.0, "display": member.display(),
                "contact_display": member.contact_display,
                "group_nickname": member.group_nickname.as_deref().unwrap_or(""),
                "is_owner": member.is_owner,
            })
        })
        .collect();
    Ok(json!({
        "chat": group.display(), "username": group.id.0, "count": members.len(), "members": members,
        "membership_complete": membership.coverage == MembershipCoverage::Complete,
        "membership_source": match membership.coverage { MembershipCoverage::Complete => "member_directory", MembershipCoverage::ObservedSenders => "observed_senders" },
    }))
}

/// 查询新消息：以 session.db 的 last_timestamp 作为 inbox 索引，
/// 只查询 last_timestamp > state[username] 的会话，精确且高效
pub async fn q_new_messages(
    db: &DbCache,
    names: &Names,
    state: Option<HashMap<String, i64>>,
    limit: usize,
    with_meta: bool,
    debug_source: bool,
) -> Result<Value> {
    message_read::new_messages(
        db,
        names,
        state,
        limit,
        MetaOptions {
            with_meta,
            debug_source,
        },
    )
    .await
}

/// Project the shared account-scoped favorite query into the JSON response.
pub async fn q_favorites(
    db: &DbCache,
    limit: usize,
    fav_type: Option<i64>,
    query: Option<String>,
) -> Result<Value> {
    use crate::{adapters::wechat::favorites as adapter, business::favorites as business};
    use business::FavoriteKind;
    let path = adapter::source_path(db).await?;
    let (rows, has_more) = tokio::task::spawn_blocking(move || {
        let mut source = adapter::Source::open(&path, fav_type)?;
        let page = business::list(&mut source, &business::Query { limit, text: query })?;
        let has_more = page.has_more;
        let rows = page
            .items
            .into_iter()
            .map(|favorite| {
                let legacy = source
                    .legacy_fields(&favorite.evidence)
                    .context("favorite provenance unavailable")?;
                let label = match favorite.kind {
                    FavoriteKind::Text => "文本",
                    FavoriteKind::Image => "图片",
                    FavoriteKind::Article => "文章",
                    FavoriteKind::ContactCard => "名片",
                    FavoriteKind::Video => "视频",
                    FavoriteKind::Other => "其他",
                };
                let preview = favorite
                    .text
                    .as_deref()
                    .map(|text| business::preview(text, 100))
                    .unwrap_or(legacy.preview);
                let mut item = json!({
                    "id": legacy.id, "type": label, "type_num": legacy.kind,
                    "favorite_id": favorite.id.0,
                    "time": fmt_time(favorite.updated_at, "%Y-%m-%d %H:%M"),
                    "timestamp": favorite.updated_at,
                    "preview": preview,
                    "from": favorite.author.unwrap_or_default(),
                    "chat": favorite.conversation.unwrap_or_default(),
                });
                if let Some(url) = favorite.article_url {
                    item["url"] = Value::String(url);
                }
                Ok::<_, anyhow::Error>(item)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok::<_, anyhow::Error>((rows, has_more))
    })
    .await??;

    Ok(json!({
        "count": rows.len(),
        "items": rows,
        "has_more": has_more,
    }))
}

/// 聊天统计：消息总数、类型分布、发言排行、24小时分布
pub async fn q_stats(
    db: &DbCache,
    names: &Names,
    chat: &str,
    since: Option<i64>,
    until: Option<i64>,
    with_meta: bool,
    debug_source: bool,
) -> Result<Value> {
    message_read::stats(
        db,
        names,
        chat,
        since,
        until,
        MetaOptions {
            with_meta,
            debug_source,
        },
    )
    .await
}

/// 查询朋友圈互动通知（点赞 + 评论），对应微信 app 右上角的红点入口。
/// 空 `content` 是点赞，非空是评论正文。
pub async fn q_sns_notifications(
    db: &DbCache,
    names: &Names,
    limit: usize,
    since: Option<i64>,
    until: Option<i64>,
    include_read: bool,
) -> Result<Value> {
    use crate::{adapters::wechat::moments as adapter, business::moments};
    let path = adapter::database_path(db).await?;
    let rows = tokio::task::spawn_blocking(move || {
        let connection = adapter::open(&path)?;
        moments::notifications(
            &mut adapter::Notifications(&connection),
            &moments::InteractionQuery {
                time: moments::TimeRange { since, until },
                include_read,
                limit,
            },
        )
    })
    .await??;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let kind = match row.kind {
            moments::InteractionKind::Like => "like",
            moments::InteractionKind::Comment => "comment",
            moments::InteractionKind::Unknown => "unknown",
        };
        let display = if row.actor_name.is_empty() {
            names.display(&row.actor)
        } else {
            row.actor_name
        };
        let author = row.original_author.unwrap_or_default();
        let author_display = if author.is_empty() {
            String::new()
        } else {
            names.display(&author)
        };
        out.push(json!({
            "type": kind, "time": fmt_time(row.created_at, "%m-%d %H:%M"), "timestamp": row.created_at,
            "from_username": row.actor, "from_nickname": display, "content": row.text,
            "feed_id": adapter::legacy_record_id(&row.moment)?,
            "feed_author_username": author, "feed_author": author_display,
            "feed_preview": row.original_preview.unwrap_or_default(),
        }));
    }
    let total = out.len();
    Ok(json!({"notifications":out,"total":total}))
}

use crate::adapters::wechat::moments::query_xml::ParsedPost;

fn post_to_value(p: ParsedPost, names: &Names) -> Value {
    let author = if p.author_username.is_empty() {
        String::new()
    } else {
        names.display(&p.author_username)
    };
    json!({
        "tid": p.tid,
        "post_id": p.post_id,
        "timestamp": p.create_time,
        "time": fmt_time(p.create_time, "%Y-%m-%d %H:%M"),
        "author_username": p.author_username,
        "author": author,
        "content": p.content,
        "media_count": p.media.len() as i64,
        "media": p.media,
        "location": p.location,
    })
}

#[derive(serde::Serialize)]
struct SnsReadStatus {
    scanned: usize,
    scan_truncated: bool,
    has_more: bool,
    unreadable: usize,
    author_conflicts: usize,
    coverage: &'static str,
}

impl From<&crate::business::moments::Page> for SnsReadStatus {
    fn from(page: &crate::business::moments::Page) -> Self {
        Self {
            scanned: page.scanned,
            scan_truncated: page.scan_truncated,
            has_more: page.more_matches,
            unreadable: page.unreadable.len(),
            author_conflicts: page.author_conflicts.len(),
            coverage: match page.coverage {
                crate::business::moments::Coverage::LocalCacheOnly => "local_cache_only",
            },
        }
    }
}

fn resolve_sns_author(query: &str, names: &Names) -> Result<String> {
    if names.map.contains_key(query) || query.starts_with("wxid_") || query.contains("@chatroom") {
        return Ok(query.to_owned());
    }
    let source =
        crate::adapters::wechat::contacts::cached_directory(&names.map, &names.verify_flags);
    crate::business::contacts::resolve(&source, query)
        .map(|contact| contact.id.0)
        .map_err(|error| match error {
            crate::business::contacts::Error::Ambiguous => {
                anyhow::Error::new(error).context(format!("ambiguous contact name: {query}"))
            }
            crate::business::contacts::Error::NotFound => anyhow::anyhow!("找不到联系人: {query}"),
            other => other.into(),
        })
}

async fn sns_moments(
    db: &DbCache,
    names: &Names,
    limit: usize,
    since: Option<i64>,
    until: Option<i64>,
    user: Option<&str>,
    keyword: Option<&str>,
) -> Result<(Vec<Value>, Option<String>, SnsReadStatus)> {
    use crate::{adapters::wechat::moments as adapter, business::moments};
    let path = adapter::database_path(db).await?;
    let author = user.map(|q| resolve_sns_author(q, names)).transpose()?;
    let resolved = author.clone();
    let query = moments::Query {
        authors: author.into_iter().collect(),
        author_policy: moments::AuthorPolicy::Effective,
        time: moments::TimeRange { since, until },
        keyword: keyword.map(str::to_owned),
        limit: limit.min(adapter::MAX_QUERY_LIMIT),
        scan_limit: adapter::MAX_QUERY_SCAN,
    };
    let (posts, status) = tokio::task::spawn_blocking(move || {
        let connection = adapter::open(&path)?;
        let mut source =
            adapter::Timeline::new(&connection, adapter::ReadPolicy::QueryCompatibility);
        let page = moments::query(&mut source, &query)?;
        let posts = page
            .moments
            .iter()
            .map(|moment| source.query_projection(moment))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok::<_, moments::SourceError>((posts, SnsReadStatus::from(&page)))
    })
    .await??;
    Ok((
        posts
            .into_iter()
            .map(|post| post_to_value(post, names))
            .collect(),
        resolved,
        status,
    ))
}

pub async fn q_sns_feed(
    db: &DbCache,
    names: &Names,
    limit: usize,
    since: Option<i64>,
    until: Option<i64>,
    user: Option<&str>,
) -> Result<Value> {
    let (posts, resolved_user, status) =
        sns_moments(db, names, limit, since, until, user, None).await?;
    let total = posts.len();
    Ok(
        json!({"posts":posts,"total":total,"resolved_user":resolved_user,
            "scanned":status.scanned,"scan_truncated":status.scan_truncated,"meta":status}),
    )
}

pub async fn q_sns_search(
    db: &DbCache,
    names: &Names,
    keyword: &str,
    limit: usize,
    since: Option<i64>,
    until: Option<i64>,
    user: Option<&str>,
) -> Result<Value> {
    if keyword.trim().is_empty() {
        anyhow::bail!("搜索关键词不能为空");
    }
    let (posts, _, status) =
        sns_moments(db, names, limit, since, until, user, Some(keyword)).await?;
    let total = posts.len();
    Ok(json!({"keyword":keyword,"posts":posts,"total":total,"meta":status}))
}

#[cfg(test)]
mod sns_business_projection_tests {
    use super::*;

    #[test]
    fn incomplete_local_scan_and_author_conflicts_survive_projection() {
        use crate::business::moments::{Coverage, EvidenceRef, Page};
        let page = Page {
            moments: vec![],
            scanned: 3,
            filtered: 0,
            unreadable: vec![EvidenceRef("unreadable".into())],
            author_conflicts: vec![EvidenceRef("conflict".into())],
            scan_truncated: true,
            more_matches: false,
            coverage: Coverage::LocalCacheOnly,
        };
        let projected = serde_json::to_value(SnsReadStatus::from(&page)).unwrap();
        assert_eq!(
            projected,
            json!({"scanned":3,"scan_truncated":true,
            "has_more":false,"unreadable":1,"author_conflicts":1,"coverage":"local_cache_only"})
        );
    }

    #[test]
    fn author_display_ambiguity_is_not_resolved_by_iteration_order() {
        let names = Names {
            map: HashMap::from([
                ("wxid_a".into(), "Same".into()),
                ("wxid_b".into(), "Same".into()),
            ]),
            msg_db_keys: vec![],
            biz_msg_db_keys: vec![],
            verify_flags: HashMap::new(),
        };
        assert!(resolve_sns_author("Same", &names)
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        assert_eq!(resolve_sns_author("wxid_a", &names).unwrap(), "wxid_a");
        assert_eq!(
            resolve_sns_author("wxid_unlisted", &names).unwrap(),
            "wxid_unlisted"
        );
        assert!(resolve_sns_author("missing display name", &names).is_err());
    }
}

// ─── 公众号文章查询 ───────────────────────────────────────────────────────────

/// Query local official-account pushes through the shared message snapshot.
pub async fn q_biz_articles(
    db: &DbCache,
    names: &Names,
    limit: usize,
    account: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    unread: bool,
) -> Result<Value> {
    use crate::{
        adapters::wechat::{articles as adapter, messages::Snapshot},
        business::{articles, messages::SourceKind},
    };
    let prepared = message_read::prepare(db, names, SourceKind::OfficialPush).await?;
    let unread_publishers = if unread {
        let path = db
            .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
            .await?
            .context("unread article source unavailable")?;
        let values = tokio::task::spawn_blocking(move || {
            let connection =
                Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            crate::adapters::wechat::messages::sessions::unread_publishers(&connection)
        })
        .await??;
        Some(
            values
                .into_iter()
                .filter(|username| chat_type_of(username, names) == "official_account")
                .collect(),
        )
    } else {
        None
    };
    let query = articles::Query {
        limit,
        publisher: account,
        received_since: since,
        received_until: until,
        unread_publishers,
    };
    let display_names = names.map.clone();
    let result = tokio::task::spawn_blocking(move || {
        let snapshot = Snapshot::open(prepared.files, display_names.keys().cloned())
            .map_err(|error| adapter::source_error(&error))?;
        let mut source = adapter::Source::new(&snapshot, &display_names);
        let page = articles::list(&mut source, &query)?;
        let partial = page.source_unfinished || !page.issues.is_empty();
        let issues: Vec<&str> = page.issues.iter().map(|issue| match issue.kind {
            articles::IssueKind::UnknownPublisher => "unknown_publisher",
            articles::IssueKind::InvalidContent => "invalid_content",
            articles::IssueKind::UnavailableSource => "unavailable_source",
            articles::IssueKind::UnsupportedSource => "unsupported_source",
            articles::IssueKind::AmbiguousIdentity => "ambiguous_identity",
        }).collect();
        let values: Vec<Value> = page.articles.into_iter().map(|article| json!({
            "time": fmt_time(article.published_at, "%Y-%m-%d %H:%M"),
            "timestamp": article.published_at,
            "recv_time": article.received_at,
            "recv_time_str": fmt_time(article.received_at, "%Y-%m-%d %H:%M"),
            "account": article.publisher_name,
            "account_username": article.publisher,
            "title": article.title,
            "url": article.url,
            "digest": article.digest,
            "cover_url": article.cover_url,
        })).collect();
        Ok::<_, anyhow::Error>(json!({
            "count": values.len(), "articles": values, "partial": partial,
            "has_more": page.has_more, "source_unfinished": page.source_unfinished, "issues": issues,
        }))
    }).await?;
    message_read::check_inventory(db, names, SourceKind::OfficialPush)?;
    result
}

/// 附件消息的内部行；分片下标只用于精确身份核验，不暴露为文件路径。
struct AttachmentRow {
    local_id: i64,
    raw_type: i64,
    timestamp: i64,
    sender: String,
    sender_username: String,
    shard_index: usize,
}

/// 普通附件列表只查询消息表；图片元数据入口另外读取资源快照，不解码附件。
pub async fn q_attachments(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: AttachmentQuery,
) -> Result<Value> {
    q_attachments_impl(db, names, chat, options, false).await
}

async fn q_attachments_impl(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: AttachmentQuery,
    image_metadata: bool,
) -> Result<Value> {
    use crate::attachment::{AttachmentId, AttachmentKind};
    let AttachmentQuery {
        kinds,
        page: MessagePage { limit, offset },
        since,
        until,
        meta: MetaOptions {
            with_meta,
            debug_source,
        },
    } = options;

    if image_metadata {
        anyhow::ensure!(limit <= 1000, "image metadata page limit exceeded");
    }

    let username = chat_identity::resolve(db, names, chat).await?;
    let display = names.display(&username);
    let chat_type = chat_type_of(&username, names);
    let is_group = chat_type == "group";

    // 解析 kinds → 低 32 bit local_type 集合
    let kind_filters: Vec<(AttachmentKind, i64)> = parse_attachment_kinds(kinds.as_deref())?;
    if kind_filters.is_empty() {
        anyhow::bail!("kinds 为空 — 当前至少传一种 image");
    }
    let lo32_types: Vec<i64> = kind_filters.iter().map(|(_, t)| *t).collect();
    // local_type → AttachmentKind 反查（mask 完后定 kind）
    let type_to_kind: HashMap<i64, AttachmentKind> =
        kind_filters.iter().map(|(k, t)| (*t, *k)).collect();

    let (shards, scanned) = find_msg_shards(db, names, &username).await?;
    if shards.is_empty() {
        anyhow::bail!("找不到 {} 的消息记录", display);
    }

    // 群聊需要 sender 显示名
    let group_nicknames = if is_group {
        load_group_nicknames(db, &username)
            .await
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    let per_db_cap = offset
        .checked_add(limit)
        .and_then(|n| n.checked_mul(2))
        .filter(|n| *n <= i64::MAX as usize)
        .context("attachment pagination overflow")?;
    let message_pins = if image_metadata {
        shards
            .iter()
            .map(|shard| crate::attachment::local_files::Pin::open(&shard.path, false))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let source_files: Vec<_> = shards
        .iter()
        .map(|shard| crate::adapters::wechat::messages::SourceFile {
            logical_name: shard.rel_key.clone(),
            path: shard.path.clone(),
            kind: crate::business::messages::SourceKind::Ordinary,
        })
        .collect();
    let needs_resource = if image_metadata && limit != 0 {
        let files = source_files.clone();
        let uname = username.clone();
        let types = lo32_types.clone();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            use crate::adapters::wechat::messages::read::attachments::AttachmentReadPolicy;
            use crate::adapters::wechat::messages::{LegacyReadPolicy, Snapshot};
            use crate::business::messages::{Filter, SourceKind};
            let snapshot = Snapshot::open(files, [uname.clone()])?;
            let mut count = 0usize;
            for stream in snapshot.streams_for(&uname, SourceKind::Ordinary) {
                count = count
                    .checked_add(
                        snapshot
                            .read_attachment_page(
                                stream,
                                &Filter {
                                    since,
                                    until,
                                    kinds: Vec::new(),
                                },
                                &LegacyReadPolicy {
                                    local_types: types.clone(),
                                },
                                per_db_cap,
                                AttachmentReadPolicy::StrictMetadata,
                            )?
                            .rows
                            .len(),
                    )
                    .context("attachment page count overflow")?;
            }
            Ok(count > offset)
        })
        .await??
    } else {
        false
    };
    let resource = if needs_resource {
        ensure_complete_message_inventory(db, names)?;
        let keys: Vec<_> = db
            .raw_db_keys()
            .into_iter()
            .filter(|key| {
                key.replace('\\', "/")
                    .eq_ignore_ascii_case("message/message_resource.db")
            })
            .collect();
        anyhow::ensure!(keys.len() <= 1, "ambiguous image resource database");
        match keys.first() {
            Some(key) => db.get(key).await?,
            None => None,
        }
    } else {
        None
    };
    let attach = image_metadata
        .then(|| {
            db.db_dir()
                .parent()
                .map(crate::attachment::resolver::attach_root_for)
                .context("missing account root")
        })
        .transpose()?;
    let source_names: Vec<_> = shards.iter().map(|shard| shard.rel_key.clone()).collect();
    let uname = username.clone();
    let names_map = names.map.clone();
    let nicknames = group_nicknames.clone();
    let (paged, metadata, shard_hits, skipped_rows, degraded_content) =
        tokio::task::spawn_blocking(move || -> Result<_> {
            use crate::adapters::wechat::messages::read::attachments::AttachmentReadPolicy;
            use crate::adapters::wechat::messages::{LegacyReadPolicy, Snapshot};
            use crate::business::media::{Error, Failure, Kind, Source, Stage};
            use crate::business::messages::{Filter, MessageSelector, SourceKind};
            let snapshot = Snapshot::open(source_files, [uname.clone()])?;
            let filter = Filter {
                since,
                until,
                kinds: Vec::new(),
            };
            let policy = if image_metadata {
                AttachmentReadPolicy::StrictMetadata
            } else {
                AttachmentReadPolicy::LegacyList
            };
            let legacy = LegacyReadPolicy {
                local_types: lo32_types,
            };
            let mut all_rows = Vec::new();
            let mut shard_hits = 0;
            let mut skipped_rows = 0usize;
            let mut degraded_content = 0usize;
            for (db_idx, name) in source_names.iter().enumerate() {
                let mut hit = false;
                for stream in snapshot.streams_for(&uname, SourceKind::Ordinary) {
                    if snapshot.source_name(stream)? != name {
                        continue;
                    }
                    let page = snapshot
                        .read_attachment_page(stream, &filter, &legacy, per_db_cap, policy)?;
                    skipped_rows += page.skipped_rows;
                    degraded_content += page.degraded_content;
                    hit |= !page.rows.is_empty();
                    for raw in page.rows {
                        let id2u = raw
                            .mapped_sender
                            .clone()
                            .map(|value| (raw.sender_id, value))
                            .into_iter()
                            .collect();
                        let sender = if is_group {
                            sender_label(
                                raw.sender_id,
                                &raw.sender_content,
                                true,
                                &uname,
                                &id2u,
                                &names_map,
                                &nicknames,
                            )
                        } else {
                            String::new()
                        };
                        let sender_username = if is_group {
                            sender_username(raw.sender_id, &raw.sender_content, true, &uname, &id2u)
                        } else {
                            String::new()
                        };
                        all_rows.push((
                            AttachmentRow {
                                local_id: raw.local_id,
                                raw_type: raw.local_type,
                                timestamp: raw.timestamp,
                                sender,
                                sender_username,
                                shard_index: db_idx,
                            },
                            raw.reference,
                        ));
                    }
                }
                shard_hits += usize::from(hit);
            }
            // Stable ties retain the original shard order and per-stream row order.
            all_rows.sort_by_key(|(row, _)| std::cmp::Reverse(row.timestamp));
            let selected: Vec<_> = all_rows.into_iter().skip(offset).take(limit).collect();
            let mut metadata = Vec::new();
            if image_metadata && !selected.is_empty() {
                let resource_snapshot = resource
                    .as_deref()
                    .map(crate::daemon::cache::ResourceSnapshot::new)
                    .transpose()?;
                let resource_path = resource_snapshot.as_ref().map(|resource| resource.path());
                let mut identities = Vec::new();
                let mut proofs = Vec::new();
                for (row, reference) in &selected {
                    snapshot.revalidate(reference)?;
                    let resolved = crate::adapters::wechat::media::image_listing_reference(
                        &snapshot,
                        &MessageSelector {
                            username: &uname,
                            local_id: row.local_id,
                            timestamp: Some(row.timestamp),
                        },
                        row.raw_type,
                    );
                    let ambiguous = match resolved {
                        Ok(unique) => {
                            anyhow::ensure!(
                                unique == *reference,
                                Error::new(Stage::Revalidation, Failure::StaleEvidence)
                            );
                            false
                        }
                        Err(error)
                            if error.downcast_ref::<crate::business::messages::Error>()
                                == Some(&crate::business::messages::Error::Ambiguous) =>
                        {
                            true
                        }
                        Err(error) => return Err(error.context("image message identity changed")),
                    };
                    identities.push((
                        crate::adapters::wechat::media::resource::MessageIdentity {
                            username: uname.clone(),
                            source: source_names[row.shard_index].clone(),
                            local_id: row.local_id,
                            create_time: row.timestamp,
                            local_type: row.raw_type,
                        },
                        ambiguous,
                    ));
                    if !ambiguous {
                        if let Some(path) = resource_path.as_deref() {
                            let mut source =
                                crate::adapters::wechat::media::ImageSource::from_reference(
                                    &snapshot, reference, path,
                                )?;
                            match source.discover(reference, Kind::Image) {
                                Ok(mut items) => {
                                    anyhow::ensure!(
                                        items.len() == 1,
                                        Error::new(Stage::Association, Failure::Ambiguous)
                                    );
                                    proofs.push((
                                        source,
                                        items.pop().expect("one image reference").reference,
                                    ));
                                }
                                Err(error)
                                    if error.stage == Stage::Association
                                        && matches!(
                                            error.failure,
                                            Failure::NotFound
                                                | Failure::Ambiguous
                                                | Failure::ConflictingEvidence
                                        ) => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                    }
                }
                metadata = crate::attachment::image_metadata::read_page(
                    resource_path.as_deref(),
                    attach.as_deref(),
                    &identities,
                )
                .map_err(|_| anyhow::anyhow!("image metadata query failed"))?;
                for (source, reference) in &proofs {
                    source.revalidate(reference)?;
                }
            }
            for (_, reference) in &selected {
                snapshot.revalidate(reference)?;
            }
            Ok((
                selected.into_iter().map(|(row, _)| row).collect::<Vec<_>>(),
                metadata,
                shard_hits,
                skipped_rows,
                degraded_content,
            ))
        })
        .await??;
    if image_metadata {
        ensure_complete_message_inventory(db, names)?;
    }
    for pin in &message_pins {
        pin.verify()?;
    }

    // 翻成 JSON
    let mut results: Vec<Value> = Vec::with_capacity(paged.len());
    for (index, item) in paged.into_iter().enumerate() {
        let AttachmentRow {
            local_id,
            raw_type,
            timestamp: ts,
            sender,
            sender_username: sender_uname,
            ..
        } = item;
        let lo32 = raw_type & 0xffff_ffff;
        let kind = type_to_kind
            .get(&lo32)
            .copied()
            .unwrap_or(AttachmentKind::Image); // 理论不会 fallthrough
        let id = AttachmentId {
            v: 1,
            chat: username.clone(),
            local_id,
            create_time: ts,
            kind,
            db: None,
        };
        let id_str = id.encode()?;

        let mut row = json!({
            "attachment_id": id_str,
            "kind": kind.as_str(),
            "type": fmt_type(lo32),
            "local_id": local_id,
            "timestamp": ts,
            "time": fmt_time(ts, "%Y-%m-%d %H:%M"),
        });
        if is_group && !sender.is_empty() {
            row["sender"] = Value::String(sender);
        }
        add_sender_identity(
            &mut row,
            is_group,
            &sender_uname,
            &names.map,
            &group_nicknames,
        );
        if image_metadata {
            let fields = serde_json::to_value(&metadata[index])?;
            row.as_object_mut()
                .context("attachment row must be object")?
                .extend(
                    fields
                        .as_object()
                        .context("image metadata must be object")?
                        .clone(),
                );
        }
        results.push(row);
    }
    let unknown_shards = current_unknown_shards(db, names);
    let session_ts = session_last_timestamp(db, &username).await;
    let meta = meta_for_shards(
        scanned,
        &shards,
        shard_hits,
        unknown_shards,
        session_ts,
        true,
        MetaOptions {
            with_meta,
            debug_source,
        },
    );

    let mut meta = serde_json::to_value(meta)?;
    if skipped_rows != 0 || degraded_content != 0 {
        if !meta.is_object() {
            meta = json!({});
        }
        meta["partial"] = json!(true);
        meta["skipped_rows"] = json!(skipped_rows);
        meta["degraded_content"] = json!(degraded_content);
    }

    Ok(json!({
        "chat": display,
        "username": username,
        "is_group": is_group,
        "chat_type": chat_type,
        "count": results.len(),
        "attachments": results,
        "meta": meta,
    }))
}

/// 图片元数据专用入口，复用普通附件查询的筛选和全局分页。
pub async fn q_attachments_with_image_metadata(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: AttachmentQuery,
) -> Result<Value> {
    q_attachments_impl(db, names, chat, options, true).await
}

#[cfg(test)]
#[path = "../../tests/fixtures/mcp-image-listing-parity/query_tests.rs"]
mod image_metadata_query_tests;

/// 解码 attachment_id → 查 message_resource.db → 找本地 .dat → 解密 → 写盘。
pub async fn q_extract(
    db: &DbCache,
    runtime: &crate::runtime::RuntimeContext,
    materials: &crate::key_store::Snapshot,
    attachment_id: &str,
    output: &str,
    overwrite: bool,
) -> Result<Value> {
    use crate::attachment::{
        attachment_id::AttachmentId,
        decoder::{self, V2KeyMaterial},
        resolver,
    };

    let id = AttachmentId::decode(attachment_id)
        .context("解析 attachment_id 失败（不是合法 base64url(json)？）")?;

    let output_path = std::path::absolute(output)?;
    let mut protected = crate::infrastructure::publication::export_protected(runtime);
    let account_root = runtime
        .config
        .db_dir
        .parent()
        .context("db_dir has no account root")?;
    protected.push(account_root.join("msg"));
    let target = if overwrite {
        crate::infrastructure::publication::ExportTarget::capture_paths(&output_path, &protected)?
    } else {
        crate::infrastructure::publication::ExportTarget::new_file(&output_path, &protected)?
    };
    let image_material = materials.image_key().map(zeroize::Zeroizing::new);

    // 1) 拿 message_resource.db
    let resource_path = db
        .get(crate::adapters::wechat::media::resource::source_key())
        .await?
        .context("无法解密资源数据库；请显式初始化当前账号数据库密钥")?;

    // 2) 推 wxchat_base = db_dir.parent()，再拼 attach_root
    let wxchat_base = db
        .db_dir()
        .parent()
        .ok_or_else(|| anyhow::anyhow!("db_dir 没有 parent，无法推断 xwechat_files 根目录"))?
        .to_path_buf();
    let attach_root = resolver::attach_root_for(&wxchat_base);

    // 3) blocking pool 跑 resolver + 读盘 + 解码
    let id_for_task = id.clone();
    let resource_path2 = resource_path.clone();
    let attach_root2 = attach_root.clone();
    let output_path2 = output_path.clone();

    let report: Value = tokio::task::spawn_blocking(move || -> Result<Value> {
        let resolved = resolver::resolve_blocking(&id_for_task, &resource_path2, &attach_root2)?;

        let source_pin = crate::attachment::local_files::Pin::open(&resolved.dat_path, false)?;
        let dat_bytes = source_pin.read_bounded(crate::attachment::native_image::MAX_DAT_BYTES)?;

        let v2_key = match image_material.as_ref() {
            Some(material) => V2KeyMaterial {
                aes_key: Some(&material.0),
                xor_key: material.1,
            },
            None => V2KeyMaterial::default(),
        };

        let decoded = decoder::restore(&dat_bytes, v2_key)?;

        target.write_bytes_checked(&decoded.data, || source_pin.verify())?;

        // 注意：不要在这里塞 `ok: true`。dispatch 会用 Response::ok(v) 包一层，
        // Response 的 `data: Value` 字段是 #[serde(flatten)] 写出的，本 payload
        // 的 `ok` 会和 Response 自带的 `ok` 在线上拼成两个同名 key，CLI 反序列化时
        // serde_json 直接报 "duplicate field"，业务请求看上去像 daemon 解析失败。
        Ok(json!({
            "kind": id_for_task.kind.as_str(),
            "md5": resolved.md5,
            "dat_path": resolved.dat_path.display().to_string(),
            "dat_size": resolved.size,
            "output": output_path2.display().to_string(),
            "output_size": decoded.data.len(),
            "format": decoded.format,
            "decoder": decoded.decoder,
        }))
    })
    .await??;

    Ok(report)
}

/// 解析 `kinds` 参数到 `(AttachmentKind, lo32_local_type)` 列表。
/// 当前只支持 image；命令名保留成 `attachments` 是为了后续扩到其他附件类型时不 break CLI。
fn parse_attachment_kinds(
    kinds: Option<&[String]>,
) -> Result<Vec<(crate::attachment::AttachmentKind, i64)>> {
    use crate::attachment::AttachmentKind;
    let raw = kinds.unwrap_or(&[]);
    if raw.is_empty() {
        return Ok(vec![(AttachmentKind::Image, 3)]);
    }
    let mut out: Vec<(AttachmentKind, i64)> = Vec::with_capacity(raw.len());
    let mut seen = HashSet::<&'static str>::new();
    for k in raw {
        let (kind, t): (AttachmentKind, i64) = match k.to_ascii_lowercase().as_str() {
            "image" => (AttachmentKind::Image, 3),
            "voice" | "audio" | "video" | "file" => {
                anyhow::bail!(
                    "当前只支持 image 提取；video/file/voice 的资源路径与 decoder 还没接通"
                )
            }
            other => anyhow::bail!("未知附件类型：{}（当前仅支持 image）", other),
        };
        if seen.insert(kind.as_str()) {
            out.push((kind, t));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod attachment_kind_tests {
    use super::parse_attachment_kinds;

    #[test]
    fn only_the_canonical_image_name_is_accepted() {
        assert!(parse_attachment_kinds(Some(&["image".into()])).is_ok());
        assert!(parse_attachment_kinds(Some(&["img".into()])).is_err());
    }
}

#[cfg(test)]
mod group_nickname_tests {
    use super::*;

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }

    fn len_field(field_no: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = varint((field_no << 3) | 2);
        out.extend(varint(bytes.len() as u64));
        out.extend(bytes);
        out
    }

    fn string_field(field_no: u64, value: &str) -> Vec<u8> {
        len_field(field_no, value.as_bytes())
    }

    fn member_chunk(username: &str, group_nickname: &str) -> Vec<u8> {
        let mut member = Vec::new();
        member.extend(string_field(1, username));
        member.extend(string_field(2, group_nickname));
        len_field(1, &member)
    }

    #[test]
    fn parses_group_nickname_member_chunks() {
        let mut ext_buffer = Vec::new();
        ext_buffer.extend(member_chunk("wxid_alice", "Alice In Group"));
        ext_buffer.extend(member_chunk("bob_123456", "Bob Card"));

        let nicknames = parse_group_nickname_map(&ext_buffer, None);

        assert_eq!(
            nicknames.get("wxid_alice").map(String::as_str),
            Some("Alice In Group")
        );
        assert_eq!(
            nicknames.get("bob_123456").map(String::as_str),
            Some("Bob Card")
        );
    }

    #[test]
    fn target_filter_anchors_member_username_choice() {
        let mut member = Vec::new();
        member.extend(string_field(3, "candidate_name"));
        member.extend(string_field(4, "wxid_target"));
        member.extend(string_field(2, "Target Card"));
        let ext_buffer = len_field(1, &member);
        let targets = HashSet::from(["wxid_target".to_string()]);

        let nicknames = parse_group_nickname_map(&ext_buffer, Some(&targets));

        assert_eq!(
            nicknames.get("wxid_target").map(String::as_str),
            Some("Target Card")
        );
        assert!(!nicknames.contains_key("candidate_name"));
    }

    #[test]
    fn ignores_non_card_string_fields_as_group_nicknames() {
        let mut ext_buffer = Vec::new();

        let mut member_without_card = Vec::new();
        member_without_card.extend(string_field(1, "wxid_alice"));
        member_without_card.extend(string_field(4, "owner_or_inviter"));
        ext_buffer.extend(len_field(1, &member_without_card));

        let mut member_with_card = Vec::new();
        member_with_card.extend(string_field(1, "wxid_bob"));
        member_with_card.extend(string_field(2, "Bob In Group"));
        member_with_card.extend(string_field(4, "owner_or_inviter"));
        ext_buffer.extend(len_field(1, &member_with_card));

        let nicknames = parse_group_nickname_map(&ext_buffer, None);

        assert!(!nicknames.contains_key("wxid_alice"));
        assert_eq!(
            nicknames.get("wxid_bob").map(String::as_str),
            Some("Bob In Group")
        );
    }

    #[test]
    fn group_top_senders_keeps_duplicate_display_names_separate() {
        let sender_counts =
            HashMap::from([("wxid_alice".to_string(), 7), ("wxid_bob".to_string(), 3)]);
        let names = HashMap::from([
            ("wxid_alice".to_string(), "Alice Contact".to_string()),
            ("wxid_bob".to_string(), "Bob Contact".to_string()),
        ]);
        let group_nicknames = HashMap::from([
            ("wxid_alice".to_string(), "同名".to_string()),
            ("wxid_bob".to_string(), "同名".to_string()),
        ]);

        let top = group_top_senders(&sender_counts, &names, &group_nicknames, 10);

        assert_eq!(top.len(), 2);
        assert_eq!(top[0]["sender"].as_str(), Some("同名"));
        assert_eq!(top[0]["sender_username"].as_str(), Some("wxid_alice"));
        assert_eq!(
            top[0]["sender_contact_display"].as_str(),
            Some("Alice Contact")
        );
        assert_eq!(top[0]["sender_group_nickname"].as_str(), Some("同名"));
        assert_eq!(top[0]["count"].as_i64(), Some(7));
        assert_eq!(top[1]["sender"].as_str(), Some("同名"));
        assert_eq!(top[1]["sender_username"].as_str(), Some("wxid_bob"));
        assert_eq!(
            top[1]["sender_contact_display"].as_str(),
            Some("Bob Contact")
        );
        assert_eq!(top[1]["sender_group_nickname"].as_str(), Some("同名"));
        assert_eq!(top[1]["count"].as_i64(), Some(3));
    }
}

#[cfg(test)]
mod sns_tests {
    use super::*;

    #[test]
    fn extract_appmsg_url_unescapes_html_entities() {
        let xml = concat!(
            "<appmsg>",
            "<type>5</type>",
            "<url>https://mp.weixin.qq.com/s?__biz=MzI4&amp;mid=2247&amp;idx=1</url>",
            "</appmsg>"
        );
        assert_eq!(
            extract_appmsg_url(xml).as_deref(),
            Some("https://mp.weixin.qq.com/s?__biz=MzI4&mid=2247&idx=1")
        );
    }

    #[test]
    fn extract_appmsg_url_strips_group_prefix_and_cdata() {
        let xml = concat!(
            "wxid_sender:\n",
            "<appmsg>",
            "<type>5</type>",
            "<url><![CDATA[https://example.com/x?a=1&b=2]]></url>",
            "</appmsg>"
        );
        assert_eq!(
            extract_appmsg_url(xml).as_deref(),
            Some("https://example.com/x?a=1&b=2")
        );
    }

    #[test]
    fn extract_appmsg_url_falls_back_to_url1() {
        let xml = concat!(
            "<appmsg>",
            "<type>5</type>",
            "<url1>https://example.com/fallback</url1>",
            "</appmsg>"
        );
        assert_eq!(
            extract_appmsg_url(xml).as_deref(),
            Some("https://example.com/fallback")
        );
    }

    #[test]
    fn extract_appmsg_url_ignores_non_http_values() {
        let xml = concat!(
            "<appmsg>",
            "<type>5</type>",
            "<url>weixin://bizmsgmenu?msgmenucontent=foo</url>",
            "</appmsg>"
        );
        assert_eq!(extract_appmsg_url(xml), None);
    }

    #[test]
    fn extract_appmsg_url_ignores_refermsg() {
        let xml = concat!(
            "<appmsg>",
            "<type>57</type>",
            "<url>https://example.com/nested</url>",
            "</appmsg>"
        );
        assert_eq!(extract_appmsg_url(xml), None);
    }

    #[test]
    fn extract_favorite_url_reads_link_tag() {
        let xml = concat!(
            "<favitem>",
            "<type>5</type>",
            "<link><![CDATA[https://mp.weixin.qq.com/s?__biz=foo&mid=1]]></link>",
            "</favitem>"
        );
        assert_eq!(
            extract_favorite_url(xml).as_deref(),
            Some("https://mp.weixin.qq.com/s?__biz=foo&mid=1")
        );
    }

    #[test]
    fn extract_favorite_url_ignores_non_http_values() {
        let xml = concat!(
            "<favitem>",
            "<type>5</type>",
            "<link>weixin://favorites/item/1</link>",
            "</favitem>"
        );
        assert_eq!(extract_favorite_url(xml), None);
    }
}
