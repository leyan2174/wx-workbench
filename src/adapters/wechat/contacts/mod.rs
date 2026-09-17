//! WeChat contact storage adapter. Paths are supplied by the fixed-account host.
use crate::business::contacts::{
    self as domain, Capabilities, Contact, ContactId, ContactKind, ContactSource, Directory, Error,
    Member, Membership, MembershipCoverage, Tag,
};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

pub(crate) mod batch;
pub(crate) mod export_alias;
mod label_values;
mod labels;
pub(crate) mod nicknames;
pub(crate) mod raw_export;
pub(crate) use labels::read_tags;
#[cfg(test)]
pub(crate) use labels::{
    MAX_ASSOCIATIONS, MAX_BUFFER_BYTES, MAX_LABELS, MAX_RESULT_TEXT_BYTES, MAX_TEXT_BYTES,
};

pub struct SqliteContacts {
    pub path: PathBuf,
    pub message_paths: Vec<PathBuf>,
    pub display_names: HashMap<String, String>,
}

/// Ordered source descriptor for cache lookup; only absence permits fallback.
pub const fn source_keys() -> [&'static str; 2] {
    ["contact/contact.db", "contact\\contact.db"]
}

impl SqliteContacts {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            message_paths: Vec::new(),
            display_names: HashMap::new(),
        }
    }
}

fn data(_: rusqlite::Error) -> Error {
    Error::InvalidData("contact database read failed")
}
fn open(path: &Path) -> domain::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| Error::Unavailable)
}
fn columns(conn: &Connection, table: &str) -> domain::Result<HashSet<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info([{table}])"))
        .map_err(data)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(data)?;
    rows.map(|row| row.map(|name| name.to_lowercase()).map_err(data))
        .collect()
}
fn has(columns: &HashSet<String>, required: &[&str]) -> bool {
    required.iter().all(|name| columns.contains(*name))
}
fn text(value: ValueRef<'_>) -> domain::Result<Option<String>> {
    match value {
        ValueRef::Null | ValueRef::Integer(0) | ValueRef::Real(0.0) | ValueRef::Blob([]) => {
            Ok(None)
        }
        ValueRef::Text(bytes) if bytes.len() <= 4096 => std::str::from_utf8(bytes)
            .map(|s| Some(s.into()))
            .map_err(|_| Error::InvalidData("contact text is not UTF-8")),
        ValueRef::Text(_) => Err(Error::Limit),
        _ => Err(Error::InvalidData("contact field must be text")),
    }
}

pub fn kind(username: &str, verified: bool) -> ContactKind {
    if username.contains("@chatroom") {
        ContactKind::Group
    } else if matches!(username, "brandsessionholder" | "@placeholder_foldgroup") {
        ContactKind::Folded
    } else if verified
        || username.starts_with("gh_")
        || username.starts_with("biz_")
        || username.starts_with('@')
    {
        ContactKind::Official
    } else {
        ContactKind::Person
    }
}

/// Adapt the host's fixed name snapshot without reopening or expanding its sources.
pub fn cached_directory(
    names: &HashMap<String, String>,
    flags: &HashMap<String, i64>,
) -> Directory {
    Directory {
        capabilities: Capabilities {
            names: true,
            classification: true,
            ..Default::default()
        },
        contacts: names
            .iter()
            .map(|(id, display)| {
                let verified = flags.get(id).copied().unwrap_or(0) != 0;
                Contact {
                    id: ContactId(id.clone()),
                    display_name: Some(display.clone()),
                    nickname: None,
                    remark: None,
                    alias: None,
                    description: None,
                    phone: None,
                    kind: kind(id, verified),
                    verified: Some(verified),
                    visible: true,
                }
            })
            .collect(),
    }
}

/// Optional display metadata for an existing connection, including caller-owned transactions.
/// Empty tables remain empty; repeated identities retain their last display value.
pub fn display_names(conn: &Connection) -> domain::Result<BTreeMap<String, String>> {
    if !has(
        &columns(conn, "contact")?,
        &["username", "nick_name", "remark"],
    ) {
        return Err(Error::Unsupported("contact names"));
    }
    let mut stmt = conn
        .prepare("SELECT username, nick_name, remark FROM contact LIMIT 100001")
        .map_err(data)?;
    let mut rows = stmt.query([]).map_err(data)?;
    let mut names = BTreeMap::new();
    let (mut count, mut bytes) = (0usize, 0usize);
    while let Some(row) = rows.next().map_err(data)? {
        count += 1;
        if count > 100_000 {
            return Err(Error::Limit);
        }
        let id = text(row.get_ref(0).map_err(data)?)?
            .filter(|id| !id.is_empty())
            .ok_or(Error::InvalidData("empty contact identity"))?;
        let nickname = text(row.get_ref(1).map_err(data)?)?;
        let remark = text(row.get_ref(2).map_err(data)?)?;
        bytes = bytes.saturating_add(
            id.len()
                + nickname.as_ref().map_or(0, String::len)
                + remark.as_ref().map_or(0, String::len),
        );
        if bytes > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let display =
            domain::preferred_name(&id, nickname.as_deref(), remark.as_deref()).to_owned();
        names.insert(id, display);
    }
    Ok(names)
}

fn capabilities(conn: &Connection) -> domain::Result<Capabilities> {
    let contact = columns(conn, "contact")?;
    if !contact.contains("username") {
        return Err(Error::Unsupported("contact identity"));
    }
    let room = columns(conn, "chat_room")?;
    let member = columns(conn, "chatroom_member")?;
    let labels = columns(conn, "contact_label")?;
    Ok(Capabilities {
        names: has(&contact, &["nick_name", "remark"]),
        classification: contact.contains("verify_flag"),
        labels: has(&labels, &["label_id_", "label_name_", "sort_order_"])
            && contact.contains("extra_buffer"),
        full_membership: has(&room, &["id", "owner"])
            && room_name(&room).is_some()
            && has(&member, &["room_id", "member_id"])
            && contact.contains("id"),
    })
}
fn room_name(columns: &HashSet<String>) -> Option<&'static str> {
    ["username", "chat_room_name", "name"]
        .into_iter()
        .find(|name| columns.contains(*name))
}

fn read_directory(conn: &Connection) -> domain::Result<Directory> {
    let capabilities = capabilities(conn)?;
    if !capabilities.names {
        return Err(Error::Unsupported("contact names"));
    }
    let columns = columns(conn, "contact")?;
    let fields = [
        "username",
        "nick_name",
        "remark",
        "alias",
        "description",
        "phone",
        "phone_number",
        "mobile",
        "mobile_phone",
        "telephone",
        "verify_flag",
        "local_type",
    ];
    let selected: Vec<_> = fields
        .iter()
        .map(|name| {
            if columns.contains(*name) {
                format!("[{name}]")
            } else {
                "NULL".into()
            }
        })
        .collect();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM contact LIMIT 100001",
            selected.join(",")
        ))
        .map_err(data)?;
    let mut rows = stmt.query([]).map_err(data)?;
    let mut contacts = Vec::new();
    let mut ids = HashSet::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().map_err(data)? {
        if contacts.len() >= 100_000 {
            return Err(Error::Limit);
        }
        let values: Vec<_> = (0..10)
            .map(|index| text(row.get_ref(index).map_err(data)?))
            .collect::<domain::Result<_>>()?;
        let id = values[0]
            .clone()
            .filter(|id| !id.is_empty())
            .ok_or(Error::InvalidData("empty contact identity"))?;
        if !ids.insert(id.clone()) {
            return Err(Error::Ambiguous);
        }
        bytes = bytes.saturating_add(values.iter().flatten().map(String::len).sum::<usize>());
        if bytes > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let verified = match row.get_ref(10).map_err(data)? {
            ValueRef::Null => false,
            ValueRef::Integer(n) => n != 0,
            _ => return Err(Error::InvalidData("invalid contact classification")),
        };
        let visible = if columns.contains("local_type") {
            match row.get_ref(11).map_err(data)? {
                ValueRef::Null => false,
                ValueRef::Integer(n) => n != 3,
                _ => return Err(Error::InvalidData("invalid contact visibility")),
            }
        } else {
            true
        };
        contacts.push(Contact {
            kind: kind(&id, verified),
            verified: capabilities.classification.then_some(verified),
            id: ContactId(id),
            display_name: None,
            nickname: values[1].clone(),
            remark: values[2].clone(),
            alias: values[3].clone(),
            description: values[4].clone(),
            phone: values[5..10]
                .iter()
                .flatten()
                .find(|s| !s.is_empty())
                .cloned(),
            visible,
        });
    }
    if contacts.is_empty() {
        return Err(Error::Unavailable);
    }
    Ok(Directory {
        contacts,
        capabilities,
    })
}

impl ContactSource for SqliteContacts {
    fn contacts(&self) -> domain::Result<Directory> {
        let conn = open(&self.path)?;
        let tx = conn.unchecked_transaction().map_err(data)?;
        read_directory(&tx)
    }
    fn tags(&self) -> domain::Result<Vec<Tag>> {
        read_tags(&self.path, &self.display_names).map_err(|error| {
            error
                .downcast_ref::<Error>()
                .cloned()
                .unwrap_or(Error::InvalidData("contact labels unavailable or invalid"))
        })
    }
    fn members(&self, group: &ContactId) -> domain::Result<Membership> {
        let conn = open(&self.path)?;
        let tx = conn.unchecked_transaction().map_err(data)?;
        let directory = read_directory(&tx)?;
        let names: HashMap<_, _> = directory
            .contacts
            .iter()
            .map(|contact| (contact.id.0.clone(), contact.display().to_owned()))
            .collect();
        let mut owner = String::new();
        let mut senders = HashSet::new();
        let mut sender_bytes = 0usize;
        let mut coverage = MembershipCoverage::ObservedSenders;
        if directory.capabilities.full_membership {
            let room_columns = columns(&tx, "chat_room")?;
            let name = room_name(&room_columns).ok_or(Error::Unsupported("group identity"))?;
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT id, owner FROM chat_room WHERE [{name}] = ? LIMIT 2"
                ))
                .map_err(data)?;
            let mut rows = stmt.query([&group.0]).map_err(data)?;
            let mut rooms = Vec::new();
            while let Some(row) = rows.next().map_err(data)? {
                rooms.push((
                    row.get::<_, i64>(0).map_err(data)?,
                    text(row.get_ref(1).map_err(data)?)?.unwrap_or_default(),
                ));
            }
            if rooms.len() > 1 {
                return Err(Error::Ambiguous);
            }
            if let Some((room_id, room_owner)) = rooms.first() {
                owner = room_owner.clone();
                let mut stmt = tx.prepare("SELECT cm.member_id, c.username FROM chatroom_member cm LEFT JOIN contact c ON c.id = cm.member_id WHERE cm.room_id = ? LIMIT 100001").map_err(data)?;
                let rows = stmt
                    .query_map([room_id], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(data)?;
                let mut linked_ids = HashSet::new();
                for row in rows {
                    let (linked_id, id) = row.map_err(data)?;
                    if !linked_ids.insert(linked_id) {
                        return Err(Error::Ambiguous);
                    }
                    if id.is_empty() {
                        return Err(Error::InvalidData("unresolved group member"));
                    }
                    sender_bytes = sender_bytes.saturating_add(id.len());
                    if !senders.insert(id) {
                        return Err(Error::Ambiguous);
                    }
                    if senders.len() > 100_000 || sender_bytes > 16 * 1024 * 1024 {
                        return Err(Error::Limit);
                    }
                }
                coverage = MembershipCoverage::Complete;
            }
        }
        if coverage == MembershipCoverage::ObservedSenders {
            if self.message_paths.is_empty() {
                return Err(Error::Unsupported("group membership"));
            }
            let table = format!("Msg_{:x}", md5::compute(group.0.as_bytes()));
            for path in &self.message_paths {
                let messages = open(path)?;
                let snapshot = messages.unchecked_transaction().map_err(data)?;
                let message_columns = columns(&snapshot, &table)?;
                if message_columns.is_empty() {
                    continue;
                }
                if !message_columns.contains("real_sender_id")
                    || !columns(&snapshot, "Name2Id")?.contains("user_name")
                {
                    return Err(Error::Unsupported("observed group senders"));
                }
                let mut stmt = snapshot.prepare(&format!("SELECT DISTINCT n.user_name FROM [{table}] m LEFT JOIN Name2Id n ON n.rowid = m.real_sender_id WHERE m.real_sender_id > 0 LIMIT 100001")).map_err(data)?;
                let mut rows = stmt.query([]).map_err(data)?;
                while let Some(row) = rows.next().map_err(data)? {
                    let id = text(row.get_ref(0).map_err(data)?)?
                        .filter(|id| !id.is_empty())
                        .ok_or(Error::InvalidData("unresolved group sender"))?;
                    let size = id.len();
                    if id != group.0 && senders.insert(id) {
                        sender_bytes = sender_bytes.saturating_add(size);
                    }
                    if senders.len() > 100_000 || sender_bytes > 16 * 1024 * 1024 {
                        return Err(Error::Limit);
                    }
                }
            }
        }
        let nicknames = nicknames::load_group_nickname_map_from_conn(&tx, &group.0, Some(&senders));
        Ok(Membership {
            coverage,
            members: senders
                .into_iter()
                .map(|id| Member {
                    contact_display: names
                        .get(&id)
                        .or_else(|| self.display_names.get(&id))
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                    group_nickname: nicknames.get(&id).cloned(),
                    is_owner: !owner.is_empty() && id == owner,
                    id: ContactId(id),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests;
