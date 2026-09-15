//! Account-scoped message contracts. Evidence references are not transport credentials.
pub mod filter_label;
pub mod reply;
pub mod statistics;
use std::{
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    Ordinary,
    OfficialPush,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Ambiguous,
    Expired,
    Unsupported,
    Unavailable,
    InvalidData,
    Limit,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFound => "message not found",
            Self::Ambiguous => "ambiguous message",
            Self::Expired => "message reference expired",
            Self::Unsupported => "unsupported message schema",
            Self::Unavailable => "message source unavailable",
            Self::InvalidData => "invalid message evidence",
            Self::Limit => "message read budget exceeded",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Private coordinates are meaningful only to the creating adapter read instance.
/// No deserializer, filesystem path, content hash, or globally registered handle.
#[derive(Clone, Debug)]
pub struct EvidenceRef {
    pub(crate) snapshot: Weak<()>,
    pub(crate) stream: usize,
    pub(crate) record: i64,
}
impl EvidenceRef {
    pub fn is_expired(&self) -> bool {
        self.snapshot.strong_count() == 0
    }
    pub(crate) fn validate(&self, owner: &Arc<()>) -> Result<()> {
        if self
            .snapshot
            .upgrade()
            .is_some_and(|value| Arc::ptr_eq(&value, owner))
        {
            Ok(())
        } else {
            Err(Error::Expired)
        }
    }
}
impl PartialEq for EvidenceRef {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.ptr_eq(&other.snapshot)
            && self.stream == other.stream
            && self.record == other.record
    }
}
impl Eq for EvidenceRef {}
impl Hash for EvidenceRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.snapshot.as_ptr().hash(state);
        self.stream.hash(state);
        self.record.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageRef(pub(crate) EvidenceRef);
impl MessageRef {
    pub fn evidence(&self) -> &EvidenceRef {
        &self.0
    }
}

/// Legacy lookup conditions, deliberately not a unique message identity.
pub struct MessageSelector<'a> {
    pub username: &'a str,
    pub local_id: i64,
    pub timestamp: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnmappedConversation(pub(crate) String);

/// An unresolved source reference is not a stable contact or conversation identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Conversation {
    Known(String),
    Unmapped(UnmappedConversation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Text,
    Image,
    Voice,
    Video,
    Call,
    Structured,
    System,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallMedia {
    Unknown,
    #[expect(
        dead_code,
        reason = "Known call audio remains distinct from Unknown; current adapter cannot prove the media type"
    )]
    Audio,
    #[expect(
        dead_code,
        reason = "Known call video remains distinct from Unknown; current adapter cannot prove the media type"
    )]
    Video,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallEvent {
    pub media: CallMedia,
    pub status_text: Option<String>,
    pub duration_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Structured(super::structured_message::StructuredMessage),
    Call(CallEvent),
    Media(Kind),
    System(String),
    Unavailable(super::structured_message::ContentIssue),
}

#[derive(Clone, Debug)]
pub struct Message {
    pub reference: MessageRef,
    pub conversation: Conversation,
    pub timestamp: i64,
    pub sender: Option<String>,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Typed message kind is independent of content and legacy localized type labels"
        )
    )]
    pub kind: Kind,
    pub call: Option<CallEvent>,
    pub content: Content,
    pub preview: String,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Incomplete inventory remains representable and is rejected by strict page selection"
        )
    )]
    Incomplete,
}

/// Pagination knowledge, independent of whether the source inventory is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageContinuation {
    Exhausted,
    /// The bounded read does not establish whether another page exists.
    MayHaveMore,
}

/// A selected page of semantic messages. Physical source and wire compatibility
/// fields belong to the creating adapter, indexed by the opaque references.
#[derive(Clone, Debug)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Source completeness is a required typed result independent of pagination and legacy wire"
        )
    )]
    pub completeness: Completeness,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Typed termination distinguishes exhausted from inconclusive bounded reads without changing legacy wire"
        )
    )]
    pub continuation: PageContinuation,
}

impl MessagePage {
    pub fn unresolved_conversations(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| matches!(message.conversation, Conversation::Unmapped(_)))
            .count()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub kinds: Vec<Kind>,
}
impl Filter {
    pub fn validate(&self) -> Result<()> {
        if self.kinds.len() > 100 {
            return Err(Error::Limit);
        }
        if self.since.zip(self.until).is_some_and(|(a, b)| a > b) {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
}

/// Source and row order are supplied by a validated snapshot, never by display names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderKey(pub i64, pub usize, pub i64);
pub struct Candidate<T> {
    pub order: OrderKey,
    pub reference: MessageRef,
    pub value: T,
}
pub struct Page {
    pub limit: usize,
    pub offset: usize,
    pub oldest_first: bool,
}
impl Page {
    /// Call after successful selection. A full page is deliberately inconclusive;
    /// a short page proves exhaustion only when all candidate reads exhausted.
    pub fn continuation(&self, returned: usize, candidates_exhausted: bool) -> PageContinuation {
        if candidates_exhausted && returned < self.limit {
            PageContinuation::Exhausted
        } else {
            PageContinuation::MayHaveMore
        }
    }

    pub fn candidate_limit(&self) -> Result<usize> {
        if self.limit == 0 {
            return Err(Error::Limit);
        }
        let size = self.offset.checked_add(self.limit).ok_or(Error::Limit)?;
        i64::try_from(size).map_err(|_| Error::Limit)?;
        Ok(size)
    }
    /// Deduplicate repeated reads of one reference only, not equal message contents.
    pub fn select<T>(
        &self,
        mut rows: Vec<Candidate<T>>,
        completeness: Completeness,
    ) -> Result<Vec<T>> {
        self.candidate_limit()?;
        if completeness != Completeness::Complete {
            return Err(Error::Unavailable);
        }
        rows.sort_by(|a, b| {
            if self.oldest_first {
                a.order.cmp(&b.order)
            } else {
                b.order
                    .0
                    .cmp(&a.order.0)
                    .then(a.order.1.cmp(&b.order.1))
                    .then(a.order.2.cmp(&b.order.2))
            }
        });
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        let mut skipped = 0;
        for row in rows {
            if row.reference.evidence().is_expired() {
                return Err(Error::Expired);
            }
            if !seen.insert(row.reference.clone()) {
                continue;
            }
            if skipped < self.offset {
                skipped += 1;
                continue;
            }
            selected.push(row);
            if selected.len() == self.limit {
                break;
            }
        }
        selected.sort_by(|a, b| a.order.cmp(&b.order));
        Ok(selected.into_iter().map(|row| row.value).collect())
    }
}

pub fn unique<T>(values: impl IntoIterator<Item = T>) -> Result<T> {
    let mut values = values.into_iter();
    let first = values.next().ok_or(Error::NotFound)?;
    if values.next().is_some() {
        Err(Error::Ambiguous)
    } else {
        Ok(first)
    }
}

pub fn matches_text(text: &str, query: &str) -> bool {
    text.contains(query)
        || (!query.is_empty() && text.to_lowercase().contains(&query.to_lowercase()))
}

/// Legacy notification subscription, not a complete history cursor. Same-second truncation
/// cannot be represented by this protocol; callers needing evidence use the message directory.
pub struct TimestampSubscription {
    pub current: std::collections::HashMap<String, i64>,
    pub previous: Option<std::collections::HashMap<String, i64>>,
    pub fallback: i64,
}
impl TimestampSubscription {
    pub fn changed(&self) -> Vec<(String, i64)> {
        let mut changed: Vec<_> = self
            .current
            .iter()
            .filter_map(|(username, timestamp)| {
                let previous = self
                    .previous
                    .as_ref()
                    .and_then(|s| s.get(username))
                    .copied()
                    .unwrap_or(self.fallback);
                (*timestamp > previous).then(|| (username.clone(), previous))
            })
            .collect();
        changed.sort_by(|a, b| a.0.cmp(&b.0));
        changed
    }
    pub fn advance(&self, delivered: &[(String, i64)]) -> std::collections::HashMap<String, i64> {
        let mut returned = std::collections::HashMap::<String, i64>::new();
        for (username, timestamp) in delivered {
            returned
                .entry(username.clone())
                .and_modify(|value| *value = (*value).max(*timestamp))
                .or_insert(*timestamp);
        }
        let mut next = self.current.clone();
        for (username, _) in self.changed() {
            let previous = self.previous.as_ref().and_then(|s| s.get(&username));
            let timestamp = returned
                .get(&username)
                .or(previous)
                .copied()
                .unwrap_or_else(|| {
                    self.current
                        .get(&username)
                        .copied()
                        .unwrap_or(self.fallback)
                });
            next.insert(username, timestamp);
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuation_is_not_source_completeness_or_pre_dedup_count() {
        let page = Page {
            limit: 2,
            offset: 0,
            oldest_first: true,
        };
        for (count, exhausted, expected) in [
            (0, true, PageContinuation::Exhausted),
            (1, true, PageContinuation::Exhausted),
            (2, true, PageContinuation::MayHaveMore),
            (0, false, PageContinuation::MayHaveMore),
            (1, false, PageContinuation::MayHaveMore),
            (2, false, PageContinuation::MayHaveMore),
        ] {
            assert_eq!(page.continuation(count, exhausted), expected);
        }
        let owner = Arc::new(());
        let candidates = [0, 0]
            .into_iter()
            .map(|record| Candidate {
                order: OrderKey(100, 0, record),
                reference: MessageRef(EvidenceRef {
                    snapshot: Arc::downgrade(&owner),
                    stream: 0,
                    record,
                }),
                value: record,
            })
            .collect();
        let selected = page.select(candidates, Completeness::Complete).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            page.continuation(selected.len(), false),
            PageContinuation::MayHaveMore
        );
        assert_eq!(
            page.continuation(selected.len(), true),
            PageContinuation::Exhausted
        );
        assert_eq!(
            page.select::<()>(vec![], Completeness::Incomplete),
            Err(Error::Unavailable)
        );
    }

    #[test]
    fn same_time_records_remain_distinct_and_repeated_reads_do_not() {
        let owner = Arc::new(());
        let reference = |record| {
            MessageRef(EvidenceRef {
                snapshot: Arc::downgrade(&owner),
                stream: 0,
                record,
            })
        };
        let rows = [2, 1, 2, 3]
            .into_iter()
            .map(|i| Candidate {
                order: OrderKey(100, 0, i),
                reference: reference(i),
                value: i,
            })
            .collect();
        assert_eq!(
            Page {
                limit: 2,
                offset: 1,
                oldest_first: true
            }
            .select(rows, Completeness::Complete)
            .unwrap(),
            [2, 3]
        );
        assert_eq!(unique([1, 1]), Err(Error::Ambiguous));
    }
    #[test]
    fn page_direction_does_not_reverse_same_second_source_or_record_ties() {
        let owner = Arc::new(());
        for oldest_first in [false, true] {
            let rows = [(1, 2), (0, 2), (1, 1), (0, 1)]
                .into_iter()
                .map(|(stream, record)| Candidate {
                    order: OrderKey(300, stream, record),
                    reference: MessageRef(EvidenceRef {
                        snapshot: Arc::downgrade(&owner),
                        stream,
                        record,
                    }),
                    value: (stream, record),
                })
                .collect();
            let page = Page {
                limit: 3,
                offset: 1,
                oldest_first,
            };
            assert_eq!(
                page.select(rows, Completeness::Complete).unwrap(),
                [(0, 2), (1, 1), (1, 2)]
            );
        }
    }

    #[test]
    fn references_never_rebind_to_another_read_instance() {
        let owner = Arc::new(());
        let reference = EvidenceRef {
            snapshot: Arc::downgrade(&owner),
            stream: 0,
            record: 1,
        };
        assert!(reference.validate(&owner).is_ok());
        assert_eq!(reference.validate(&Arc::new(())), Err(Error::Expired));
        drop(owner);
        assert!(reference.is_expired());
    }
    #[test]
    fn timestamp_subscription_keeps_legacy_truncation_rules() {
        use std::collections::HashMap;
        let mut subscription = TimestampSubscription {
            current: HashMap::from([("a".into(), 100), ("b".into(), 200)]),
            previous: None,
            fallback: 0,
        };
        assert_eq!(subscription.advance(&[("a".into(), 50)])["b"], 200);
        subscription.previous = Some(HashMap::from([("a".into(), 10), ("b".into(), 20)]));
        let next = subscription.advance(&[("a".into(), 50)]);
        assert_eq!((next["a"], next["b"]), (50, 20));
    }
}
