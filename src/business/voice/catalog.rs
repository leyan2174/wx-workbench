//! Account-scoped voice previews. Source evidence is not a message ID or authorization.
use std::{any::Any, collections::HashMap, fmt, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    EmptyUsername,
    EmptyChat,
    AmbiguousChat,
    EmptyLimit,
    InvalidRange,
    PaginationOverflow,
    InvalidPage,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Catalog source failure remains distinct from an empty successful page"
        )
    )]
    Unavailable,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyUsername => "explicit username required",
            Self::EmptyChat => "chat must not be empty",
            Self::AmbiguousChat => "chat has no unique exact match; use explicit username",
            Self::EmptyLimit => "limit must be positive",
            Self::InvalidRange => "since exceeds until",
            Self::PaginationOverflow => "pagination overflow",
            Self::InvalidPage => "invalid voice catalog page",
            Self::Unavailable => "voice catalog unavailable",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug)]
pub struct Query {
    pub username: String,
    pub limit: usize,
    pub offset: usize,
    pub since: Option<i64>,
    pub until: Option<i64>,
}
impl Query {
    /// Call after a successful complete-inventory read. No lookahead is spent.
    pub fn continuation(&self, returned: usize) -> PageContinuation {
        if returned < self.limit {
            PageContinuation::Exhausted
        } else {
            PageContinuation::MayHaveMore
        }
    }

    pub fn candidate_limit(&self) -> Result<usize, Error> {
        if self.username.trim().is_empty() {
            return Err(Error::EmptyUsername);
        }
        if self.limit == 0 {
            return Err(Error::EmptyLimit);
        }
        if self.since.zip(self.until).is_some_and(|(a, b)| a > b) {
            return Err(Error::InvalidRange);
        }
        self.offset
            .checked_add(self.limit)
            .ok_or(Error::PaginationOverflow)
    }
}

/// Adapter-owned diagnostic evidence. Equality only compares this preview's
/// evidence handle; it does not identify messages across reads or accounts.
#[derive(Clone)]
pub struct SourceRef(Arc<dyn Any + Send + Sync>);
impl SourceRef {
    pub(crate) fn new<T: Any + Send + Sync>(evidence: T) -> Self {
        Self(Arc::new(evidence))
    }
    pub(crate) fn evidence<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}
impl fmt::Debug for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourceRef(<opaque>)")
    }
}
impl PartialEq for SourceRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for SourceRef {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub username: String,
    pub timestamp: i64,
    /// None is unknown; Some(0) is a known empty payload.
    pub byte_len: Option<u64>,
    pub source: SourceRef,
}

/// Same semantics as messages::PageContinuation, kept local so this narrow
/// capability does not depend on the message-reading model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageContinuation {
    Exhausted,
    /// A full bounded page does not prove that another page exists.
    MayHaveMore,
}

/// Success requires a complete source inventory, independently of pagination.
/// No total or cross-database atomic snapshot is claimed.
#[derive(Clone, Debug)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub offset: usize,
    pub limit: usize,
    pub continuation: PageContinuation,
}

pub trait Source {
    type Error: From<Error>;
    /// Preserve source ordering and duplicate observations. The host supplies
    /// a complete account inventory; the source must not skip unavailable items.
    fn read(&self, query: &Query) -> Result<Page, Self::Error>;
}

pub fn list<S: Source>(source: &S, query: &Query) -> Result<Page, S::Error> {
    query.candidate_limit()?;
    let page = source.read(query)?;
    if page.offset != query.offset
        || page.limit != query.limit
        || page.entries.len() > query.limit
        || page.continuation != query.continuation(page.entries.len())
        || page.entries.iter().any(|entry| {
            entry.username != query.username
                || query.since.is_some_and(|time| entry.timestamp < time)
                || query.until.is_some_and(|time| entry.timestamp > time)
        })
    {
        return Err(Error::InvalidPage.into());
    }
    Ok(page)
}

pub fn resolve_exact_chat(chat: &str, names: &HashMap<String, String>) -> Result<String, Error> {
    if chat.trim().is_empty() {
        return Err(Error::EmptyChat);
    }
    if names.contains_key(chat) {
        return Ok(chat.into());
    }
    let matches: Vec<_> = names
        .iter()
        .filter(|(_, display)| display.as_str() == chat)
        .map(|(user, _)| user)
        .collect();
    if matches.len() != 1 {
        return Err(Error::AmbiguousChat);
    }
    Ok(matches[0].clone())
}

#[cfg(test)]
mod tests;
