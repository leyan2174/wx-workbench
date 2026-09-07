//! 显式离线快照中的语音关联。仅返回原始字节及证据，不解码、不上传、不写文件。
use rusqlite::{Connection, OpenFlags};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

pub const MAX_VOICE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MEDIA_SHARDS: usize = 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidIdentity,
    UnsafePath,
    ActiveDatabase,
    MissingDatabase,
    UnsupportedSchema,
    InvalidMessage,
    NotVoice,
    MessageNotFound,
    AmbiguousMessage,
    AmbiguousContact,
    MediaNotFound,
    AmbiguousMedia,
    ConflictingEvidence,
    InvalidVoiceData,
    LimitExceeded,
    DatabaseRead,
    SnapshotChanged,
}

/// 错误不包含原始 SQL、聊天内容或绝对路径，调用方可按 kind 分类处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseMediaError {
    pub kind: ErrorKind,
    pub stage: &'static str,
}

impl fmt::Display for DatabaseMediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.stage)
    }
}
impl std::error::Error for DatabaseMediaError {}
type Result<T> = std::result::Result<T, DatabaseMediaError>;

fn error(kind: ErrorKind, stage: &'static str) -> DatabaseMediaError {
    DatabaseMediaError { kind, stage }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageIdentity<'a> {
    pub username: &'a str,
    /// 必须是完整根相对来源：message/message_N.db，接受 Windows 分隔符。
    pub source: &'a str,
    pub local_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceEvidence {
    pub username: String,
    pub message_source: String,
    pub message_table: String,
    pub message_local_id: i64,
    pub server_id: i64,
    pub create_time: i64,
    pub media_source: String,
    pub media_rowid: i64,
    pub media_chat_name_id: i64,
    /// 与 message_local_id 不必相等，禁止用于反推消息分片。
    pub media_local_id: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseVoice {
    /// 保留原始 SILK 字节，包括微信可能存在的 0x02 前缀。
    pub silk: Vec<u8>,
    pub evidence: VoiceEvidence,
}

impl fmt::Debug for DatabaseVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseVoice")
            .field("silk_bytes", &self.silk.len())
            .field("evidence", &self.evidence)
            .finish()
    }
}

/// 显式账号清单的一项；source 是原始根相对键，path 是对应的静态解密文件。
#[derive(Debug, Clone)]
pub struct DecryptedSource {
    pub source: String,
    pub path: PathBuf,
}

/// 保留旧目录入口；目录清单和显式路径清单共用同一关联实现。
pub fn resolve_voice(
    decrypted_root: &Path,
    identity: MessageIdentity<'_>,
) -> Result<DatabaseVoice> {
    let root = snapshot_root(decrypted_root)?;
    let directory = root.join("message");
    let media_paths = media_shards(&directory)?;
    let source = canonical_source(identity.source)?;
    if !source.starts_with("message/message_") {
        return Err(error(ErrorKind::InvalidIdentity, "message source"));
    }
    let mut sources = vec![DecryptedSource {
        source: source.clone(),
        path: root.join(&source),
    }];
    sources.extend(media_paths.iter().map(|name| DecryptedSource {
        source: format!("message/{name}"),
        path: directory.join(name),
    }));
    let result = resolve_voice_sources(&sources, identity, None);
    if media_shards(&directory)? != media_paths {
        return Err(error(ErrorKind::SnapshotChanged, "media shard set"));
    }
    result
}

/// 调用方保证显式清单完整且属于同一账号；不发现目录、不读取账号配置。
/// 接受散列缓存路径，拒绝重复来源、文件别名和活动库；所有源均持有只读保护。
/// exact_time=None 不按时间筛选，Some(0) 则精确匹配零时间戳。
pub fn resolve_voice_sources(
    sources: &[DecryptedSource],
    identity: MessageIdentity<'_>,
    exact_time: Option<i64>,
) -> Result<DatabaseVoice> {
    if identity.username.is_empty()
        || identity.username.len() > 1024
        || identity.username.chars().any(char::is_control)
        || identity.local_id <= 0
    {
        return Err(error(ErrorKind::InvalidIdentity, "username/local_id"));
    }
    let source = canonical_source(identity.source)?;
    if !source.starts_with("message/message_") {
        return Err(error(ErrorKind::InvalidIdentity, "message source"));
    }
    with_sources(sources, |opened, media| {
        let message = opened
            .get(&source)
            .ok_or_else(|| error(ErrorKind::MissingDatabase, "message source absent"))?;
        join_voice(message, media, identity, &source, exact_time)
    })
}

/// 两种消息身份入口共用清单校验、只读句柄和结束复核，不复制数据库打开逻辑。
fn with_sources<T>(
    sources: &[DecryptedSource],
    resolve: impl FnOnce(&std::collections::BTreeMap<String, ReadDb>, &[(&str, &ReadDb)]) -> Result<T>,
) -> Result<T> {
    if sources.len() > 2 * MAX_MEDIA_SHARDS + 1 {
        return Err(error(ErrorKind::LimitExceeded, "database source count"));
    }
    let mut opened = std::collections::BTreeMap::new();
    let mut handles = std::collections::HashSet::new();
    for entry in sources {
        let key = canonical_source(&entry.source)?;
        if opened.contains_key(&key) {
            return Err(error(
                ErrorKind::InvalidIdentity,
                "duplicate canonical source",
            ));
        }
        if !entry.path.is_absolute()
            || entry
                .path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(error(ErrorKind::UnsafePath, "source must be absolute"));
        }
        for ancestor in entry.path.ancestors() {
            reject_reparse(ancestor)?;
        }
        let handle = same_file::Handle::from_path(&entry.path)
            .map_err(|_| error(ErrorKind::DatabaseRead, "source file identity"))?;
        if !handles.insert(handle) {
            return Err(error(
                ErrorKind::UnsafePath,
                "database sources alias the same file",
            ));
        }
        let db = ReadDb::open(&entry.path)?;
        db.conn
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|_| error(ErrorKind::DatabaseRead, "source schema"))?;
        opened.insert(key, db);
    }
    let media: Vec<_> = opened
        .iter()
        .filter(|(key, _)| key.starts_with("message/media_"))
        .map(|(key, db)| (key.as_str(), db))
        .collect();
    if media.is_empty() {
        return Err(error(ErrorKind::MissingDatabase, "no media shards"));
    }
    // 先检查每个媒体库的证据表，不能让前片歧义掩盖后片的不支持模式。
    for (_, db) in &media {
        require_table(&db.conn, "Name2Id", &["user_name"])?;
        require_table(
            &db.conn,
            "VoiceInfo",
            &[
                "chat_name_id",
                "local_id",
                "create_time",
                "svr_id",
                "voice_data",
            ],
        )?;
    }
    let result = resolve(&opened, &media);
    for db in opened.values() {
        db.verify()?;
    }
    result
}

/// 旧公开 local_id 明确指 VoiceInfo.local_id，绝不作为消息表 local_id 使用。
/// 先证明唯一媒体行，再按 username/server_id 反查所有消息分片；最后复用正向关联。
/// 歧义以 DatabaseMediaError.kind 返回，不返回未经验证的字节或猜测候选。
pub fn resolve_voice_media_id(
    sources: &[DecryptedSource],
    username: &str,
    media_local_id: i64,
) -> Result<DatabaseVoice> {
    if username.is_empty() || username.len() > 1024 || username.chars().any(char::is_control) {
        return Err(error(ErrorKind::InvalidIdentity, "username"));
    }
    with_sources(sources, |opened, media| {
        let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
        let mut messages = Vec::new();
        for (source, db) in opened
            .iter()
            .filter(|(s, _)| s.starts_with("message/message_"))
        {
            let exists: bool = db
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 COLLATE NOCASE)",
                    [&table],
                    |r| r.get(0),
                )
                .map_err(|_| error(ErrorKind::DatabaseRead, "message schema"))?;
            if exists {
                require_table(
                    &db.conn,
                    &table,
                    &["local_id", "local_type", "create_time", "server_id"],
                )?;
                messages.push((source.as_str(), db));
            }
        }
        let mut candidate = None;
        let mut ambiguous_media = false;
        for (source, db) in media {
            let Some(chat_id) = contact_id(&db.conn, username)? else {
                continue;
            };
            let mut statement = db.conn.prepare(
                "SELECT rowid,local_id,create_time,svr_id,chat_name_id FROM VoiceInfo WHERE chat_name_id=?1 AND local_id=?2 LIMIT 2",
            ).map_err(|_| error(ErrorKind::DatabaseRead, "legacy media query"))?;
            let mut rows = statement
                .query([chat_id, media_local_id])
                .map_err(|_| error(ErrorKind::DatabaseRead, "legacy media rows"))?;
            while let Some(row) = rows
                .next()
                .map_err(|_| error(ErrorKind::DatabaseRead, "legacy media row"))?
            {
                if candidate.is_some() {
                    ambiguous_media = true;
                    continue;
                }
                let rowid: i64 = row
                    .get(0)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media rowid"))?;
                let proven_id: i64 = row
                    .get(1)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media local_id"))?;
                let time: i64 = row
                    .get(2)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media time"))?;
                let server: i64 = row
                    .get(3)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media server_id"))?;
                let proven_chat: i64 = row
                    .get(4)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media chat_name_id"))?;
                if proven_id != media_local_id || proven_chat != chat_id {
                    return Err(error(
                        ErrorKind::ConflictingEvidence,
                        "legacy media identity mismatch",
                    ));
                }
                candidate = Some((*source, rowid, time, server));
            }
        }
        if ambiguous_media {
            return Err(error(
                ErrorKind::AmbiguousMedia,
                "duplicate legacy media local_id",
            ));
        }
        let (media_source, media_rowid, media_time, server) =
            candidate.ok_or_else(|| error(ErrorKind::MediaNotFound, "legacy media local_id"))?;
        if server == 0 {
            return Err(error(
                ErrorKind::ConflictingEvidence,
                "zero server_id cannot prove reverse join",
            ));
        }
        let mut message = None;
        let mut ambiguous_message = false;
        for (source, db) in messages {
            let mut statement = db.conn.prepare(&format!(
                "SELECT local_id,create_time,local_type,server_id FROM [{table}] WHERE server_id=?1 LIMIT 2"
            )).map_err(|_| error(ErrorKind::DatabaseRead, "reverse message query"))?;
            let mut rows = statement
                .query([server])
                .map_err(|_| error(ErrorKind::DatabaseRead, "reverse message rows"))?;
            while let Some(row) = rows
                .next()
                .map_err(|_| error(ErrorKind::DatabaseRead, "reverse message row"))?
            {
                if message.is_some() {
                    ambiguous_message = true;
                    continue;
                }
                let id: i64 = row
                    .get(0)
                    .map_err(|_| error(ErrorKind::InvalidMessage, "message local_id"))?;
                let time: i64 = row
                    .get(1)
                    .map_err(|_| error(ErrorKind::InvalidMessage, "message time"))?;
                let kind: i64 = row
                    .get(2)
                    .map_err(|_| error(ErrorKind::InvalidMessage, "message type"))?;
                let proven_server: i64 = row
                    .get(3)
                    .map_err(|_| error(ErrorKind::InvalidMessage, "message server_id"))?;
                if proven_server != server {
                    return Err(error(
                        ErrorKind::ConflictingEvidence,
                        "reverse server_id mismatch",
                    ));
                }
                message = Some((source, db, id, time, kind));
            }
        }
        if ambiguous_message {
            return Err(error(
                ErrorKind::AmbiguousMessage,
                "multiple messages match media server_id",
            ));
        }
        let (source, db, id, time, kind) = message.ok_or_else(|| {
            error(
                ErrorKind::MessageNotFound,
                "no message proves legacy media ownership",
            )
        })?;
        if id <= 0 {
            return Err(error(
                ErrorKind::InvalidMessage,
                "message local_id must be positive",
            ));
        }
        if kind < 0 {
            return Err(error(ErrorKind::NotVoice, "message type"));
        }
        let voice = join_voice(
            db,
            media,
            MessageIdentity {
                username,
                source,
                local_id: id,
            },
            source,
            Some(time),
        )?;
        if voice.evidence.media_source != media_source
            || voice.evidence.media_rowid != media_rowid
            || voice.evidence.media_local_id != media_local_id
            || voice.evidence.create_time != media_time
        {
            return Err(error(
                ErrorKind::ConflictingEvidence,
                "reverse and forward media evidence differ",
            ));
        }
        Ok(voice)
    })
}

fn canonical_source(value: &str) -> Result<String> {
    let value = value.replace('\\', "/").to_ascii_lowercase();
    let valid = value == "contact/contact.db"
        || value
            .strip_prefix("message/")
            .is_some_and(|name| numbered_db(name, "message_") || numbered_db(name, "media_"));
    if !valid {
        return Err(error(ErrorKind::InvalidIdentity, "invalid database source"));
    }
    Ok(value)
}

/// 唯一的消息与媒体关联实现；目录和缓存适配均调用这里。
fn join_voice(
    message: &ReadDb,
    media: &[(&str, &ReadDb)],
    identity: MessageIdentity<'_>,
    source: &str,
    exact_time: Option<i64>,
) -> Result<DatabaseVoice> {
    let table = format!("Msg_{:x}", md5::compute(identity.username.as_bytes()));
    require_table(
        &message.conn,
        &table,
        &["local_id", "local_type", "create_time", "server_id"],
    )?;
    // 消息归属由实际 Msg_<md5(username)> 表及精确分片共同证明，不按昵称解析。
    let mut stmt = message
        .conn
        .prepare(&format!(
            "SELECT local_type, create_time, server_id, local_id FROM [{table}] WHERE local_id=?1 AND (?2 IS NULL OR create_time=?2) LIMIT 2"
        ))
        .map_err(|_| error(ErrorKind::DatabaseRead, "message query"))?;
    let mut rows = stmt
        .query(rusqlite::params![identity.local_id, exact_time])
        .map_err(|_| error(ErrorKind::DatabaseRead, "message rows"))?;
    let row = rows
        .next()
        .map_err(|_| error(ErrorKind::DatabaseRead, "message row"))?
        .ok_or_else(|| error(ErrorKind::MessageNotFound, "exact source/table/local_id"))?;
    let local_type: i64 = row
        .get(0)
        .map_err(|_| error(ErrorKind::InvalidMessage, "local_type"))?;
    let create_time: i64 = row
        .get(1)
        .map_err(|_| error(ErrorKind::InvalidMessage, "create_time"))?;
    let server_id: i64 = row
        .get(2)
        .map_err(|_| error(ErrorKind::InvalidMessage, "server_id"))?;
    let proven_local_id: i64 = row
        .get(3)
        .map_err(|_| error(ErrorKind::InvalidMessage, "local_id must be an integer"))?;
    if proven_local_id != identity.local_id {
        return Err(error(
            ErrorKind::ConflictingEvidence,
            "message local_id mismatch",
        ));
    }
    if rows
        .next()
        .map_err(|_| error(ErrorKind::DatabaseRead, "message uniqueness"))?
        .is_some()
    {
        return Err(error(
            ErrorKind::AmbiguousMessage,
            "duplicate message local_id",
        ));
    }
    if local_type & 0xffff_ffff != 34 {
        return Err(error(ErrorKind::NotVoice, "message type"));
    }
    if server_id == 0 {
        return Err(error(
            ErrorKind::ConflictingEvidence,
            "zero server_id cannot prove join",
        ));
    }

    let mut match_data = None;
    for (media_source, db) in media {
        let chat_id = contact_id(&db.conn, identity.username)?;
        if let Some(chat_id) = chat_id {
            let mut stmt = db.conn.prepare(
                "SELECT rowid, local_id, create_time, typeof(voice_data), length(voice_data), svr_id, chat_name_id FROM VoiceInfo WHERE chat_name_id=?1 AND svr_id=?2 LIMIT 2"
            ).map_err(|_| error(ErrorKind::UnsupportedSchema, "VoiceInfo rowid/query"))?;
            let mut rows = stmt
                .query([chat_id, server_id])
                .map_err(|_| error(ErrorKind::DatabaseRead, "media query"))?;
            if let Some(row) = rows
                .next()
                .map_err(|_| error(ErrorKind::DatabaseRead, "media row"))?
            {
                if match_data.is_some() {
                    return Err(error(
                        ErrorKind::AmbiguousMedia,
                        "multiple media shards match",
                    ));
                }
                let rowid: i64 = row
                    .get(0)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media rowid"))?;
                let media_local_id: i64 = row
                    .get(1)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media local_id"))?;
                let media_time: i64 = row
                    .get(2)
                    .map_err(|_| error(ErrorKind::ConflictingEvidence, "media time"))?;
                let kind: String = row
                    .get(3)
                    .map_err(|_| error(ErrorKind::InvalidVoiceData, "media storage type"))?;
                let length: Option<i64> = row
                    .get(4)
                    .map_err(|_| error(ErrorKind::InvalidVoiceData, "media length"))?;
                let proven_server: i64 = row.get(5).map_err(|_| {
                    error(
                        ErrorKind::ConflictingEvidence,
                        "media svr_id must be an integer",
                    )
                })?;
                let proven_chat: i64 = row.get(6).map_err(|_| {
                    error(
                        ErrorKind::ConflictingEvidence,
                        "media chat_name_id must be an integer",
                    )
                })?;
                if proven_server != server_id || proven_chat != chat_id {
                    return Err(error(
                        ErrorKind::ConflictingEvidence,
                        "media join identity mismatch",
                    ));
                }
                if rows
                    .next()
                    .map_err(|_| error(ErrorKind::DatabaseRead, "media uniqueness"))?
                    .is_some()
                {
                    return Err(error(ErrorKind::AmbiguousMedia, "duplicate VoiceInfo rows"));
                }
                if media_time != create_time {
                    return Err(error(
                        ErrorKind::ConflictingEvidence,
                        "message/media create_time differ",
                    ));
                }
                if kind != "blob" || !length.is_some_and(|n| n > 0 && n <= MAX_VOICE_BYTES as i64) {
                    return Err(error(
                        ErrorKind::InvalidVoiceData,
                        "empty/non-BLOB/oversized voice",
                    ));
                }
                // 长度检查先于 BLOB 取值；不把未知或巨大的媒体内容装进内存。
                let silk: Vec<u8> = db
                    .conn
                    .query_row(
                        "SELECT voice_data FROM VoiceInfo WHERE rowid=?1",
                        [rowid],
                        |row| row.get(0),
                    )
                    .map_err(|_| error(ErrorKind::DatabaseRead, "voice bytes"))?;
                let header = silk.strip_prefix(&[2]).unwrap_or(&silk);
                if !header.starts_with(b"#!SILK_V3") {
                    return Err(error(ErrorKind::InvalidVoiceData, "SILK_V3 header"));
                }
                match_data = Some(DatabaseVoice {
                    silk,
                    evidence: VoiceEvidence {
                        username: identity.username.into(),
                        message_source: source.to_owned(),
                        message_table: table.clone(),
                        message_local_id: identity.local_id,
                        server_id,
                        create_time,
                        media_source: (*media_source).to_owned(),
                        media_rowid: rowid,
                        media_chat_name_id: chat_id,
                        media_local_id,
                    },
                });
            }
        }
    }
    match_data.ok_or_else(|| {
        error(
            ErrorKind::MediaNotFound,
            "username/server_id in all provided media shards",
        )
    })
}

fn numbered_db(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(".db"))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn snapshot_root(decrypted_root: &Path) -> Result<PathBuf> {
    if !decrypted_root.is_absolute()
        || decrypted_root
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(error(ErrorKind::UnsafePath, "root must be absolute"));
    }
    for ancestor in decrypted_root.ancestors() {
        reject_reparse(ancestor)?;
    }
    decrypted_root
        .canonicalize()
        .map_err(|_| error(ErrorKind::MissingDatabase, "root"))
}

/// 仅枚举静态快照的受控源路径，不打开 SQLite、不读取正文或音频。
/// Name2Id 联系人依赖位于媒体库；另保护存在时的固定 contact/contact.db。
/// 调用方须先完成上传授权，并保证快照及父目录在操作期间保持稳定。
pub fn source_files(decrypted_root: &Path) -> Result<Vec<PathBuf>> {
    let root = snapshot_root(decrypted_root)?;
    let directory = root.join("message");
    let mut paths = Vec::new();
    for prefix in ["message_", "media_"] {
        paths.extend(
            shard_names(&directory, prefix)?
                .into_iter()
                .map(|name| directory.join(name)),
        );
    }
    let contact = root.join("contact");
    match fs::symlink_metadata(&contact) {
        Ok(_) => {
            reject_reparse(&contact)?;
            let path = contact.join("contact.db");
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    reject_regular_file(&path)?;
                    paths.push(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(error(ErrorKind::DatabaseRead, "contact metadata")),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error(ErrorKind::DatabaseRead, "contact directory metadata")),
    }
    let mut identities = std::collections::HashSet::new();
    for path in &paths {
        let identity = same_file::Handle::from_path(path)
            .map_err(|_| error(ErrorKind::DatabaseRead, "source file identity"))?;
        if !identities.insert(identity) {
            return Err(error(
                ErrorKind::UnsafePath,
                "database sources alias the same file",
            ));
        }
    }
    Ok(paths)
}

fn media_shards(directory: &Path) -> Result<Vec<String>> {
    let names = shard_names(directory, "media_")?;
    if names.is_empty() {
        return Err(error(ErrorKind::MissingDatabase, "no media shards"));
    }
    Ok(names)
}

fn shard_names(directory: &Path, prefix: &str) -> Result<Vec<String>> {
    reject_reparse(directory)?;
    let mut names = Vec::new();
    let mut sources = std::collections::BTreeSet::new();
    for (index, entry) in fs::read_dir(directory)
        .map_err(|_| error(ErrorKind::MissingDatabase, "message directory"))?
        .enumerate()
    {
        if index >= MAX_DIRECTORY_ENTRIES {
            return Err(error(
                ErrorKind::LimitExceeded,
                "message directory entry count",
            ));
        }
        let entry = entry.map_err(|_| error(ErrorKind::DatabaseRead, "list media shards"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| error(ErrorKind::UnsafePath, "non-UTF8 database filename"))?;
        // 识别和证据规范化不区分大小写；打开文件及快照复核仍保留原名。
        let canonical = name.to_ascii_lowercase();
        if canonical.starts_with(prefix) && canonical.ends_with(".db") {
            if !numbered_db(&canonical, prefix) {
                return Err(error(
                    ErrorKind::UnsupportedSchema,
                    "unrecognized media shard name",
                ));
            }
            reject_regular_file(&entry.path())?;
            if !sources.insert(canonical) {
                return Err(error(
                    ErrorKind::AmbiguousMedia,
                    "duplicate canonical media source",
                ));
            }
            for previous in &names {
                if same_file::is_same_file(directory.join(previous), entry.path())
                    .map_err(|_| error(ErrorKind::DatabaseRead, "media file identity"))?
                {
                    return Err(error(
                        ErrorKind::AmbiguousMedia,
                        "media sources alias the same file",
                    ));
                }
            }
            names.push(name);
            if names.len() > MAX_MEDIA_SHARDS {
                return Err(error(ErrorKind::LimitExceeded, "media shard count"));
            }
        }
    }
    names.sort();
    Ok(names)
}

fn reject_regular_file(path: &Path) -> Result<()> {
    reject_reparse(path)?;
    if !fs::symlink_metadata(path)
        .map_err(|_| error(ErrorKind::DatabaseRead, "source metadata"))?
        .is_file()
    {
        return Err(error(
            ErrorKind::UnsafePath,
            "database must be regular file",
        ));
    }
    Ok(())
}

fn require_table(conn: &Connection, table: &str, columns: &[&str]) -> Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND type='table' AND wr=0 AND name=?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| error(ErrorKind::DatabaseRead, "schema catalog"))?;
    if !exists {
        return Err(error(
            ErrorKind::UnsupportedSchema,
            "required ordinary rowid table absent",
        ));
    }
    // xinfo 包含生成列及隐藏列，不能让它们遮蔽 SQLite 内部行号。
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_xinfo(?1, 'main')")
        .map_err(|_| error(ErrorKind::UnsupportedSchema, "table_xinfo"))?;
    let found = stmt
        .query_map([table], |row| row.get::<_, String>(0))
        .map_err(|_| error(ErrorKind::DatabaseRead, "table columns"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| error(ErrorKind::DatabaseRead, "table column names"))?;
    if columns
        .iter()
        .any(|required| !found.iter().any(|c| c.eq_ignore_ascii_case(required)))
    {
        return Err(error(
            ErrorKind::UnsupportedSchema,
            "required evidence column absent",
        ));
    }
    // rowid 必须是 SQLite 自身的行标识，不能被同名用户列冒充。
    if found.iter().any(|name| {
        ["rowid", "_rowid_", "oid"]
            .iter()
            .any(|alias| name.eq_ignore_ascii_case(alias))
    }) {
        return Err(error(ErrorKind::UnsupportedSchema, "shadowed SQLite rowid"));
    }
    Ok(())
}

fn contact_id(conn: &Connection, username: &str) -> Result<Option<i64>> {
    let mut stmt = conn
        .prepare("SELECT rowid FROM Name2Id WHERE user_name COLLATE BINARY = ?1 LIMIT 2")
        .map_err(|_| error(ErrorKind::UnsupportedSchema, "Name2Id rowid"))?;
    let ids = stmt
        .query_map([username], |row| row.get::<_, i64>(0))
        .map_err(|_| error(ErrorKind::DatabaseRead, "Name2Id query"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| error(ErrorKind::DatabaseRead, "Name2Id result"))?;
    match ids.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        _ => Err(error(
            ErrorKind::AmbiguousContact,
            "duplicate Name2Id username",
        )),
    }
}

fn reject_reparse(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| error(ErrorKind::MissingDatabase, "path metadata"))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(error(ErrorKind::UnsafePath, "reparse point"));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(error(ErrorKind::UnsafePath, "symbolic link"));
    }
    Ok(())
}

fn reject_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(_) => {
                return Err(error(
                    ErrorKind::ActiveDatabase,
                    "WAL/SHM/journal must be absent",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(error(ErrorKind::DatabaseRead, "sidecar metadata")),
        }
    }
    Ok(())
}

struct ReadDb {
    conn: Connection,
    path: PathBuf,
    // Windows 上持有禁止写入和删除的源句柄，直到关联检查完成。
    _guard: fs::File,
    length: u64,
    modified: std::time::SystemTime,
}

impl ReadDb {
    fn open(path: &Path) -> Result<Self> {
        reject_reparse(path)?;
        reject_sidecars(path)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let guard = options
            .open(path)
            .map_err(|_| error(ErrorKind::DatabaseRead, "lock read-only snapshot"))?;
        let metadata = guard
            .metadata()
            .map_err(|_| error(ErrorKind::DatabaseRead, "snapshot metadata"))?;
        if !metadata.is_file() {
            return Err(error(
                ErrorKind::UnsafePath,
                "database must be regular file",
            ));
        }
        // immutable 禁止 SQLite 创建 sidecar；只接受无 WAL 的静态副本，不能用于活动库。
        let raw = path
            .to_str()
            .ok_or_else(|| error(ErrorKind::UnsafePath, "non-UTF8 path"))?;
        let raw = raw.strip_prefix(r"\\?\").unwrap_or(raw).replace('\\', "/");
        if raw.starts_with("UNC/") || raw.starts_with("//") {
            return Err(error(
                ErrorKind::UnsafePath,
                "network snapshots unsupported",
            ));
        }
        let mut uri = String::from("file:///");
        for byte in raw.trim_start_matches('/').bytes() {
            if byte.is_ascii_alphanumeric() || b"/-_.:".contains(&byte) {
                uri.push(byte as char);
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
        uri.push_str("?mode=ro&immutable=1");
        let conn = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| error(ErrorKind::DatabaseRead, "immutable SQLite open"))?;
        conn.pragma_update(None, "query_only", true)
            .map_err(|_| error(ErrorKind::DatabaseRead, "query_only"))?;
        Ok(Self {
            conn,
            path: path.into(),
            _guard: guard,
            length: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|_| error(ErrorKind::DatabaseRead, "snapshot modified time"))?,
        })
    }

    fn verify(&self) -> Result<()> {
        reject_sidecars(&self.path)?;
        let current = fs::metadata(&self.path)
            .map_err(|_| error(ErrorKind::SnapshotChanged, "snapshot disappeared"))?;
        if current.len() != self.length || current.modified().ok() != Some(self.modified) {
            return Err(error(ErrorKind::SnapshotChanged, "snapshot modified"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "database_media_tests.rs"]
mod tests;
