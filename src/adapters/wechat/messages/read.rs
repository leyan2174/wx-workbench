//! Read instances over an explicit account inventory. No discovery, keys, or host configuration.
use crate::business::messages::{
    self as domain, Conversation, EvidenceRef, MessageRef, MessageSelector, SourceKind,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::Arc,
};

pub const MAX_STORED_BYTES: usize = 1_048_576;
pub const MAX_DECODED_BYTES: usize = 4 * 1_048_576;
const MAX_STREAMS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaIssue {
    ShadowedSqliteRowid,
}
impl std::fmt::Display for SchemaIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShadowedSqliteRowid => f.write_str("shadowed SQLite rowid"),
        }
    }
}
impl std::error::Error for SchemaIssue {}

fn strict_row_key(cols: &BTreeSet<String>) -> Result<&'static str> {
    if ["rowid", "_rowid_", "oid"]
        .iter()
        .any(|key| cols.contains(*key))
    {
        return Err(anyhow::Error::new(domain::Error::Unsupported)
            .context(SchemaIssue::ShadowedSqliteRowid));
    }
    Ok("rowid")
}

/// Only the account host supplies paths. Logical names are checked independently of paths.
#[derive(Clone)]
pub struct SourceFile {
    pub logical_name: String,
    pub path: PathBuf,
    pub kind: SourceKind,
}
/// Numeric WeChat filters are a legacy protocol policy, not business message kinds.
#[derive(Default)]
pub struct LegacyReadPolicy {
    pub local_types: Vec<i64>,
}
pub struct Stream {
    source: usize,
    table: String,
    row_key: &'static str,
    columns: BTreeSet<String>,
    pub conversation: Conversation,
}
impl Stream {
    pub fn table_name(&self) -> &str {
        &self.table
    }
    pub fn supports_identity(&self) -> bool {
        self.columns.contains("local_id")
    }
    pub fn supports_senders(&self) -> bool {
        self.columns.contains("real_sender_id")
    }
    pub fn supports_content(&self) -> bool {
        self.columns.contains("message_content") && self.columns.contains("wcdb_ct_message_content")
    }
    pub fn supports_server_id(&self) -> bool {
        self.columns.contains("server_id")
    }
}
struct Source {
    logical_name: String,
    kind: SourceKind,
    conn: Connection,
    senders: BTreeMap<i64, String>,
}

/// All connections keep their own SQLite read transaction until this value is dropped.
/// This is not a simultaneous cross-database transaction or a persistent cursor service.
pub struct Snapshot {
    owner: Arc<()>,
    sources: Vec<Source>,
    streams: Vec<Stream>,
}

#[derive(Clone, Debug)]
pub enum StoredContent {
    AbsentColumn,
    NotRead,
    Null,
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
/// Content captured while a source read was valid. No live identity or revalidation promise.
pub struct DetachedContent {
    pub compression: Option<i64>,
    pub content: StoredContent,
}
#[derive(Clone, Debug, PartialEq)]
pub enum StoredScalar {
    AbsentColumn,
    Null,
    Integer(i64),
    Text(String),
    Real(f64),
    Blob(Vec<u8>),
}
fn scalar(value: ValueRef<'_>) -> Result<StoredScalar> {
    Ok(match value {
        ValueRef::Null => StoredScalar::Null,
        ValueRef::Integer(value) => StoredScalar::Integer(value),
        ValueRef::Real(value) => StoredScalar::Real(value),
        ValueRef::Text(bytes) => {
            ensure!(bytes.len() <= 4096, domain::Error::Limit);
            StoredScalar::Text(std::str::from_utf8(bytes)?.to_owned())
        }
        ValueRef::Blob(bytes) => {
            ensure!(bytes.len() <= 4096, domain::Error::Limit);
            StoredScalar::Blob(bytes.to_vec())
        }
    })
}
#[derive(Clone, Debug)]
pub struct RawMessage {
    pub reference: MessageRef,
    pub logical_source: String,
    pub local_id: Option<i64>,
    pub local_type: i64,
    pub timestamp: i64,
    pub sender_id: Option<i64>,
    pub sender: Option<String>,
    pub compression: Option<i64>,
    pub content: StoredContent,
    pub server_id: StoredScalar,
    pub sort_seq: StoredScalar,
}
impl RawMessage {
    pub fn detached_content(&self) -> DetachedContent {
        DetachedContent {
            compression: self.compression,
            content: self.content.clone(),
        }
    }
    /// Legacy ASR accepts only actual SQLite INTEGER identifiers; local_id is never a substitute.
    pub fn checked_server_id(&self) -> Result<Option<i64>> {
        match &self.server_id {
            StoredScalar::AbsentColumn | StoredScalar::Null | StoredScalar::Integer(0) => {
                Err(domain::Error::InvalidData).context("nonzero server_id evidence required")
            }
            StoredScalar::Integer(value) => Ok(Some(*value)),
            _ => {
                Err(domain::Error::InvalidData).context("server_id must be SQLite INTEGER or NULL")
            }
        }
    }
    pub fn bounded_decode(&self, limit: usize) -> Result<Vec<u8>> {
        decode_content(&self.content, self.compression, limit)
    }
}
impl DetachedContent {
    pub fn bounded_decode(&self, limit: usize) -> Result<Vec<u8>> {
        decode_content(&self.content, self.compression, limit)
    }
}
pub(super) fn decode_content(
    content: &StoredContent,
    compression: Option<i64>,
    limit: usize,
) -> Result<Vec<u8>> {
    match content {
        StoredContent::AbsentColumn | StoredContent::NotRead => {
            anyhow::bail!(domain::Error::Unsupported)
        }
        StoredContent::Null => anyhow::bail!(domain::Error::InvalidData),
        StoredContent::Blob(bytes) if compression == Some(4) => {
            let mut result = Vec::new();
            zstd::stream::read::Decoder::new(bytes.as_slice())?
                .take(
                    u64::try_from(limit)?
                        .checked_add(1)
                        .context("decode budget overflow")?,
                )
                .read_to_end(&mut result)?;
            ensure!(
                result.len() <= limit,
                "message body exceeds decoded byte limit"
            );
            Ok(result)
        }
        StoredContent::Text(bytes) | StoredContent::Blob(bytes) => {
            ensure!(
                bytes.len() <= limit,
                "message body exceeds decoded byte limit"
            );
            Ok(bytes.clone())
        }
    }
}

fn valid_table(name: &str) -> bool {
    name.to_ascii_lowercase()
        .strip_prefix("msg_")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
}
pub(super) fn logical_name(raw: &str, kind: SourceKind) -> Result<String> {
    let name = raw.replace('\\', "/").to_ascii_lowercase();
    let prefix = match kind {
        SourceKind::Ordinary => "message/message_",
        SourceKind::OfficialPush => "message/biz_message_",
    };
    ensure!(
        name.strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(".db"))
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())),
        domain::Error::InvalidData
    );
    Ok(name)
}
fn columns(conn: &Connection, table: &str) -> Result<BTreeSet<String>> {
    Ok(conn
        .prepare(&format!("PRAGMA table_xinfo([{table}])"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .map(|row| row.map(|s| s.to_ascii_lowercase()))
        .collect::<rusqlite::Result<_>>()?)
}
fn add_name(names: &mut BTreeMap<String, String>, username: &str) -> Result<()> {
    ensure!(username.len() <= 4096, domain::Error::Limit);
    if username.is_empty() {
        return Ok(());
    }
    let hash = format!("{:x}", md5::compute(username.as_bytes()));
    if let Some(previous) = names.insert(hash, username.to_owned()) {
        ensure!(previous == username, domain::Error::Ambiguous);
    }
    Ok(())
}

pub fn read_senders(conn: &Connection) -> Result<BTreeMap<i64, String>> {
    let mut senders = BTreeMap::new();
    let schema: Option<(String, i64)> = {
        use rusqlite::OptionalExtension;
        conn.query_row("SELECT type,wr FROM pragma_table_list WHERE schema='main' AND name='Name2Id' COLLATE NOCASE", [], |r| Ok((r.get(0)?, r.get(1)?))).optional()?
    };
    let Some((kind, wr)) = schema else {
        return Ok(senders);
    };
    ensure!(kind == "table" && wr == 0, domain::Error::Unsupported);
    let cols = columns(conn, "Name2Id")?;
    ensure!(cols.contains("user_name"), domain::Error::Unsupported);
    let row_key = strict_row_key(&cols)?;
    let mut statement = conn.prepare(&format!(
        "SELECT {row_key},user_name FROM Name2Id LIMIT 100001"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        ensure!(senders.len() < MAX_STREAMS, domain::Error::Limit);
        let bytes = match row.get_ref(1)? {
            ValueRef::Text(bytes) => bytes,
            _ => anyhow::bail!(domain::Error::InvalidData),
        };
        ensure!(bytes.len() <= 4096, domain::Error::Limit);
        senders.insert(row.get(0)?, std::str::from_utf8(bytes)?.to_owned());
    }
    Ok(senders)
}

impl Snapshot {
    pub fn open(
        files: Vec<SourceFile>,
        usernames: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        ensure!(!files.is_empty(), domain::Error::Unavailable);
        ensure!(files.len() <= 20_000, domain::Error::Limit);
        let mut files: Vec<_> = files
            .into_iter()
            .map(|file| Ok((logical_name(&file.logical_name, file.kind)?, file)))
            .collect::<Result<_>>()?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        ensure!(
            files.windows(2).all(|p| p[0].0 != p[1].0),
            domain::Error::Ambiguous
        );
        let mut names = BTreeMap::new();
        for username in usernames {
            add_name(&mut names, &username)?;
        }
        let mut sources = Vec::new();
        let mut streams = Vec::new();
        for (_, file) in files {
            let logical_name = file.logical_name.clone();
            let conn = Connection::open_with_flags(&file.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("cannot read message shard: {logical_name}"))?;
            conn.execute_batch("BEGIN DEFERRED")?;
            let objects: Vec<(String, String, i64)> = conn
                .prepare(
                    "SELECT name,type,wr FROM pragma_table_list WHERE schema='main' ORDER BY name",
                )
                .context("unsupported message table schema")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()
                .context(domain::Error::Unsupported)
                .context("unsupported message table schema")?;
            let senders = read_senders(&conn)?;
            for username in senders.values() {
                add_name(&mut names, username)?;
            }
            for (table, kind, wr) in objects
                .into_iter()
                .filter(|(name, _, _)| name.to_ascii_lowercase().starts_with("msg_"))
            {
                if !valid_table(&table) || kind != "table" || wr != 0 {
                    return Err(anyhow::Error::new(domain::Error::Unsupported)
                        .context("unsupported message table schema"));
                }
                let cols = columns(&conn, &table)?;
                for column in ["local_type", "create_time"] {
                    if !cols.contains(column) {
                        return Err(anyhow::Error::new(domain::Error::Unsupported).context(
                            format!("unsupported message table schema: missing {column}"),
                        ));
                    }
                }
                let row_key = strict_row_key(&cols)?;
                ensure!(streams.len() < MAX_STREAMS, domain::Error::Limit);
                streams.push(Stream {
                    source: sources.len(),
                    conversation: Conversation::Unmapped(table[4..].to_ascii_lowercase()),
                    table,
                    row_key,
                    columns: cols,
                });
            }
            sources.push(Source {
                logical_name,
                kind: file.kind,
                conn,
                senders,
            });
        }
        for stream in &mut streams {
            if let Some(username) = names.get(&stream.table[4..].to_ascii_lowercase()) {
                stream.conversation = Conversation::Known(username.clone());
            }
        }
        Ok(Self {
            owner: Arc::new(()),
            sources,
            streams,
        })
    }
    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }
    pub fn source_name(&self, stream: usize) -> Result<&str> {
        let stream = self.streams.get(stream).context("unknown message stream")?;
        Ok(&self.sources[stream.source].logical_name)
    }
    pub fn source_kind(&self, stream: usize) -> Result<SourceKind> {
        let stream = self.streams.get(stream).context("unknown message stream")?;
        Ok(self.sources[stream.source].kind)
    }
    pub fn conversation(&self, reference: &MessageRef) -> Result<&Conversation> {
        reference.evidence().validate(&self.owner)?;
        Ok(&self
            .streams
            .get(reference.0.stream)
            .context("unknown message stream")?
            .conversation)
    }
    pub fn latest_timestamp(&self, stream: usize) -> Result<Option<i64>> {
        let stream = self.streams.get(stream).context("unknown message stream")?;
        Ok(self.sources[stream.source].conn.query_row(
            &format!("SELECT MAX(create_time) FROM [{}]", stream.table),
            [],
            |r| r.get(0),
        )?)
    }
    pub fn streams_for(&self, username: &str, kind: SourceKind) -> Vec<usize> {
        let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
        self.streams
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.table.eq_ignore_ascii_case(&table) && self.sources[s.source].kind == kind
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn reference(&self, stream: usize, record: i64) -> MessageRef {
        MessageRef(EvidenceRef {
            snapshot: Arc::downgrade(&self.owner),
            stream,
            record,
        })
    }
    /// Validate the requested projection even when a known target table has no rows.
    pub fn require_target_content(&self, username: &str, kind: SourceKind) -> Result<()> {
        for stream in self.streams_for(username, kind) {
            self.require_content(stream)?;
        }
        Ok(())
    }

    fn require_content(&self, stream: usize) -> Result<()> {
        let stream = self.streams.get(stream).context("unknown message stream")?;
        if !stream.supports_content() {
            return Err(anyhow::Error::new(domain::Error::Unsupported)
                .context("unsupported message table schema: message_content and WCDB_CT_message_content required"));
        }
        Ok(())
    }

    pub fn resolve(&self, selector: &MessageSelector<'_>, kind: SourceKind) -> Result<MessageRef> {
        let mut matches = Vec::new();
        for stream in self.streams_for(selector.username, kind) {
            let entry = &self.streams[stream];
            ensure!(entry.supports_identity(), domain::Error::Unsupported);
            let sql = format!(
                "SELECT {} FROM [{}] WHERE local_id=?1 AND (?2 IS NULL OR create_time=?2) LIMIT 2",
                entry.row_key, entry.table
            );
            let rows: Vec<i64> = self.sources[entry.source]
                .conn
                .prepare(&sql)?
                .query_map(
                    rusqlite::params![selector.local_id, selector.timestamp],
                    |r| r.get(0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            // Continue checking every source even after ambiguity: a broken later source must fail.
            for row in rows {
                if matches.len() < 2 {
                    matches.push(self.reference(stream, row));
                }
            }
        }
        Ok(domain::unique(matches)?)
    }
    pub fn revalidate(&self, reference: &MessageRef) -> Result<()> {
        let evidence = reference.evidence();
        evidence.validate(&self.owner)?;
        let stream = self
            .streams
            .get(evidence.stream)
            .context("unknown message stream")?;
        let exists: bool = self.sources[stream.source].conn.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM [{}] WHERE {}=?1)",
                stream.table, stream.row_key
            ),
            [evidence.record],
            |r| r.get(0),
        )?;
        ensure!(exists, domain::Error::Expired);
        Ok(())
    }
    /// Legacy media selector, not a durable ID. No fallback to local_id or another timestamp.
    pub fn resolve_server_id(
        &self,
        username: &str,
        server_id: i64,
        timestamp: Option<i64>,
        kind: SourceKind,
    ) -> Result<MessageRef> {
        ensure!(server_id != 0, domain::Error::InvalidData);
        let mut matches = Vec::new();
        for stream in self.streams_for(username, kind) {
            let entry = &self.streams[stream];
            ensure!(entry.supports_server_id(), domain::Error::Unsupported);
            let sql = format!(
                "SELECT {},server_id FROM [{}] WHERE server_id=?1 LIMIT 2",
                entry.row_key, entry.table
            );
            let mut statement = self.sources[entry.source].conn.prepare(&sql)?;
            let mut rows = statement.query([server_id])?;
            while let Some(row) = rows.next()? {
                ensure!(
                    matches!(row.get_ref(1)?, ValueRef::Integer(value) if value == server_id),
                    domain::Error::InvalidData
                );
                if matches.len() < 2 {
                    matches.push(self.reference(stream, row.get(0)?));
                }
            }
        }
        let reference = domain::unique(matches)?;
        if let Some(timestamp) = timestamp {
            ensure!(
                self.read_metadata(reference.evidence())?.timestamp == timestamp,
                domain::Error::InvalidData
            );
        }
        Ok(reference)
    }
    pub fn read_evidence(&self, reference: &EvidenceRef) -> Result<RawMessage> {
        self.read_record(reference, true)
    }
    /// Identity and server association do not require or decode message body columns.
    pub fn read_metadata(&self, reference: &EvidenceRef) -> Result<RawMessage> {
        self.read_record(reference, false)
    }
    fn read_record(&self, reference: &EvidenceRef, with_content: bool) -> Result<RawMessage> {
        reference.validate(&self.owner)?;
        let stream = self
            .streams
            .get(reference.stream)
            .context("unknown message stream")?;
        let source = &self.sources[stream.source];
        if with_content {
            self.require_content(reference.stream)?;
        }
        let local_id = if stream.supports_identity() {
            "local_id"
        } else {
            "NULL"
        };
        let sender = if stream.supports_senders() {
            "real_sender_id"
        } else {
            "NULL"
        };
        let server_id = if stream.columns.contains("server_id") {
            "server_id"
        } else {
            "NULL"
        };
        let sort_seq = if stream.columns.contains("sort_seq") {
            "sort_seq"
        } else {
            "NULL"
        };
        let body = if with_content {
            "WCDB_CT_message_content,length(CAST(message_content AS BLOB)),message_content"
        } else {
            "NULL,NULL,NULL"
        };
        let sql = format!("SELECT {local_id},local_type,create_time,{sender},{body},{server_id},{sort_seq} FROM [{}] WHERE {}=?1", stream.table, stream.row_key);
        let mut statement = source.conn.prepare(&sql)?;
        let mut rows = statement.query([reference.record])?;
        let row = rows.next()?.ok_or(domain::Error::Expired)?;
        let length: Option<i64> = row.get(5)?;
        ensure!(
            length.unwrap_or(0) <= MAX_STORED_BYTES as i64,
            "message body exceeds stored byte limit"
        );
        let content = if !with_content {
            if stream.supports_content() {
                StoredContent::NotRead
            } else {
                StoredContent::AbsentColumn
            }
        } else {
            match row.get_ref(6)? {
                ValueRef::Null => StoredContent::Null,
                ValueRef::Text(bytes) => {
                    std::str::from_utf8(bytes).context("SQLite TEXT is not valid UTF-8")?;
                    StoredContent::Text(bytes.to_vec())
                }
                ValueRef::Blob(bytes) => StoredContent::Blob(bytes.to_vec()),
                _ => anyhow::bail!(domain::Error::InvalidData),
            }
        };
        let sender_id = row.get::<_, Option<i64>>(3)?;
        Ok(RawMessage {
            reference: MessageRef(reference.clone()),
            logical_source: source.logical_name.clone(),
            local_id: row.get(0)?,
            local_type: row.get(1)?,
            timestamp: row.get(2)?,
            sender_id,
            sender: sender_id
                .and_then(|id| source.senders.get(&id))
                .filter(|s| !s.is_empty())
                .cloned(),
            compression: row.get(4)?,
            content,
            server_id: if stream.supports_server_id() {
                scalar(row.get_ref(7)?)?
            } else {
                StoredScalar::AbsentColumn
            },
            sort_seq: if stream.columns.contains("sort_seq") {
                scalar(row.get_ref(8)?)?
            } else {
                StoredScalar::AbsentColumn
            },
        })
    }
    pub fn read_page(
        &self,
        stream: usize,
        filter: &domain::Filter,
        limit: usize,
        oldest_first: bool,
    ) -> Result<Vec<RawMessage>> {
        self.read_legacy_page(
            stream,
            filter,
            &LegacyReadPolicy::default(),
            limit,
            oldest_first,
        )
    }
    pub fn read_legacy_page(
        &self,
        stream: usize,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        limit: usize,
        oldest_first: bool,
    ) -> Result<Vec<RawMessage>> {
        self.read_selection(stream, filter, legacy, limit, oldest_first, None, None)
    }
    pub(super) fn read_selection(
        &self,
        stream: usize,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        limit: usize,
        oldest_first: bool,
        keyword: Option<&str>,
        preview: Option<&dyn Fn(&RawMessage) -> Result<String>>,
    ) -> Result<Vec<RawMessage>> {
        filter.validate()?;
        ensure!(legacy.local_types.len() <= 100, domain::Error::Limit);
        self.require_content(stream)?;
        let entry = self.streams.get(stream).context("unknown message stream")?;
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        if let Some(since) = filter.since {
            clauses.push("create_time >= ?".to_owned());
            values.push(since);
        }
        if let Some(until) = filter.until {
            clauses.push("create_time <= ?".to_owned());
            values.push(until);
        }
        if !filter.kinds.is_empty() {
            clauses.push(format!(
                "({})",
                filter
                    .kinds
                    .iter()
                    .map(|kind| match kind {
                        domain::Kind::Text => "(local_type & 4294967295) = 1",
                        domain::Kind::Image => "(local_type & 4294967295) = 3",
                        domain::Kind::Voice => "(local_type & 4294967295) = 34",
                        domain::Kind::Video => "(local_type & 4294967295) IN (43,62)",
                        domain::Kind::Call => "(local_type & 4294967295) = 50",
                        domain::Kind::Structured => "(local_type & 4294967295) = 49",
                        domain::Kind::System => "(local_type & 4294967295) IN (10000,10002)",
                        domain::Kind::Unknown =>
                            "(local_type & 4294967295) NOT IN (1,3,34,43,62,49,50,10000,10002)",
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ));
        }
        if !legacy.local_types.is_empty() {
            clauses.push(format!(
                "({})",
                legacy
                    .local_types
                    .iter()
                    .map(|kind| if (0..=u32::MAX as i64).contains(kind) {
                        "(local_type & 4294967295) = ?"
                    } else {
                        "local_type = ?"
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ));
            values.extend(&legacy.local_types);
        }
        // Preserve the legacy distinction: app-message search examines decoded text and preview.
        let decoded_search = keyword.is_some() && legacy.local_types == [49];
        let mut parameters: Vec<rusqlite::types::Value> =
            values.into_iter().map(Into::into).collect();
        if let Some(keyword) = keyword.filter(|_| !decoded_search) {
            clauses.push("message_content LIKE ? ESCAPE '\\'".to_owned());
            parameters.push(
                format!(
                    "%{}%",
                    keyword
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
                .into(),
            );
        }
        let condition = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let order = if oldest_first { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT {} FROM [{}] {condition} ORDER BY create_time {order},{} ASC LIMIT ?",
            entry.row_key, entry.table, entry.row_key
        );
        let requested = i64::try_from(limit).map_err(|_| domain::Error::Limit)?;
        parameters.push(
            if decoded_search {
                100_001
            } else {
                requested.min(100_001)
            }
            .into(),
        );
        let ids: Vec<i64> = self.sources[entry.source]
            .conn
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(parameters), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        for (position, id) in ids.into_iter().enumerate() {
            ensure!(position < 100_000, domain::Error::Limit);
            let row = self.read_evidence(self.reference(stream, id).evidence())?;
            let stored = match &row.content {
                StoredContent::AbsentColumn | StoredContent::NotRead | StoredContent::Null => 0,
                StoredContent::Text(b) | StoredContent::Blob(b) => b.len(),
            };
            bytes = bytes
                .checked_add(stored)
                .context("message read budget overflow")?;
            ensure!(bytes <= 64 * 1_048_576, domain::Error::Limit);
            if let Some(keyword) = keyword.filter(|_| decoded_search) {
                let decoded = row.bounded_decode(MAX_DECODED_BYTES)?;
                let text = std::str::from_utf8(&decoded)?;
                if !domain::matches_text(text, keyword)
                    && !domain::matches_text(
                        &preview.context("decoded search projection unavailable")?(&row)?,
                        keyword,
                    )
                {
                    continue;
                }
            }
            result.push(row);
            if result.len() == limit {
                break;
            }
        }
        Ok(result)
    }
    pub fn order_key(&self, message: &RawMessage) -> Result<domain::OrderKey> {
        message.reference.evidence().validate(&self.owner)?;
        Ok(domain::OrderKey(
            message.timestamp,
            message.reference.0.stream,
            message.reference.0.record,
        ))
    }
}
