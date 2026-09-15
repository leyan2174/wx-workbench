//! Account-scoped local moments. Sources are injected; this module performs no I/O.
use std::{collections::BTreeSet, error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceRef(pub String);

/// A host-published local media file, independent of cache layout and source format.
pub struct RecoveredMediaFile {
    pub relative_path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorOrigin {
    Recorded,
    EmbeddedFallback,
    Conflict,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    pub recorded: Option<String>,
    pub embedded: Option<String>,
    pub effective: Option<String>,
    pub origin: AuthorOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorPolicy {
    Effective,
    RecordedCompatibility,
}

impl Author {
    pub fn identity(&self, policy: AuthorPolicy) -> Option<&str> {
        match policy {
            AuthorPolicy::Effective => self.effective.as_deref(),
            AuthorPolicy::RecordedCompatibility => self.recorded.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaRef {
    pub evidence: EvidenceRef,
    pub identity: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentQuality {
    Parsed,
    RecoveredText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    pub id: String,
    pub evidence: EvidenceRef,
    pub author: Author,
    pub created_at: i64,
    pub text: String,
    pub media: Vec<MediaRef>,
    pub quality: ContentQuality,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Unavailable,
    UnsupportedFormat,
    InvalidData,
    AmbiguousIdentity,
    InvalidQuery,
}
impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Moments source unavailable",
            Self::UnsupportedFormat => "Moments source format unsupported",
            Self::InvalidData => "Moments source contains invalid data",
            Self::AmbiguousIdentity => "Moments source identity is ambiguous",
            Self::InvalidQuery => "Invalid moments query",
        })
    }
}
impl Error for SourceError {}

#[derive(Clone, Copy, Debug, Default)]
pub struct TimeRange {
    pub since: Option<i64>,
    pub until: Option<i64>,
}
impl TimeRange {
    pub fn contains(self, timestamp: i64) -> bool {
        self.since.is_none_or(|s| timestamp >= s) && self.until.is_none_or(|u| timestamp <= u)
    }
}

pub struct Query {
    pub authors: BTreeSet<String>,
    pub author_policy: AuthorPolicy,
    pub time: TimeRange,
    pub keyword: Option<String>,
    pub limit: usize,
    pub scan_limit: usize,
}

impl Query {
    pub fn matches_author(&self, author: &Author) -> bool {
        self.authors.is_empty()
            || author
                .identity(self.author_policy)
                .is_some_and(|id| self.authors.contains(id))
    }
}

pub struct Candidate {
    pub evidence: EvidenceRef,
    pub author: Author,
    pub moment: Option<Moment>,
}
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub scanned: usize,
    pub truncated: bool,
}

/// The keyword is a semantic search request, never a source expression or predicate.
/// Sources preserve their explicitly documented encoded-search compatibility policy.
pub trait TimelineSource {
    fn scan(&mut self, query: &Query) -> Result<Scan, SourceError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    LocalCacheOnly,
}

pub struct Page {
    pub moments: Vec<Moment>,
    pub scanned: usize,
    pub filtered: usize,
    pub unreadable: Vec<EvidenceRef>,
    pub author_conflicts: Vec<EvidenceRef>,
    pub scan_truncated: bool,
    pub more_matches: bool,
    pub coverage: Coverage,
}

pub fn query(source: &mut impl TimelineSource, query: &Query) -> Result<Page, SourceError> {
    if query.keyword.as_ref().is_some_and(|s| s.trim().is_empty()) {
        return Err(SourceError::InvalidQuery);
    }
    let scan = source.scan(query)?;
    let mut page = Page {
        scanned: scan.scanned,
        filtered: 0,
        moments: Vec::new(),
        unreadable: Vec::new(),
        author_conflicts: Vec::new(),
        scan_truncated: scan.truncated,
        more_matches: false,
        coverage: Coverage::LocalCacheOnly,
    };
    for candidate in scan.candidates {
        if !query.matches_author(&candidate.author) {
            page.filtered += 1;
            continue;
        }
        if candidate.author.origin == AuthorOrigin::Conflict {
            page.author_conflicts.push(candidate.evidence.clone());
        }
        let Some(moment) = candidate.moment else {
            page.unreadable.push(candidate.evidence);
            continue;
        };
        if query.time.contains(moment.created_at) {
            page.moments.push(moment);
        } else {
            page.filtered += 1;
        }
    }
    // Stable ordering preserves source order among equal timestamps.
    page.moments
        .sort_by_key(|p| std::cmp::Reverse(p.created_at));
    page.more_matches = page.moments.len() > query.limit;
    page.moments.truncate(query.limit);
    Ok(page)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InteractionKind {
    Like,
    Comment,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct Interaction {
    pub evidence: EvidenceRef,
    pub moment: EvidenceRef,
    pub created_at: i64,
    pub actor: String,
    pub actor_name: String,
    pub text: String,
    pub kind: InteractionKind,
    pub unread: bool,
    pub original_author: Option<String>,
    pub original_preview: Option<String>,
}
pub struct InteractionQuery {
    pub time: TimeRange,
    pub include_read: bool,
    pub limit: usize,
}
pub trait InteractionSource {
    fn interactions(&mut self, query: &InteractionQuery) -> Result<Vec<Interaction>, SourceError>;
}
pub fn notifications(
    source: &mut impl InteractionSource,
    query: &InteractionQuery,
) -> Result<Vec<Interaction>, SourceError> {
    let mut rows = source.interactions(query)?;
    rows.retain(|row| (query.include_read || row.unread) && query.time.contains(row.created_at));
    rows.sort_by_key(|row| std::cmp::Reverse(row.created_at));
    rows.truncate(query.limit);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory(Vec<Candidate>);
    impl TimelineSource for Memory {
        fn scan(&mut self, query: &Query) -> Result<Scan, SourceError> {
            let mut candidates = std::mem::take(&mut self.0);
            let truncated = candidates.len() > query.scan_limit;
            candidates.truncate(query.scan_limit);
            Ok(Scan {
                scanned: candidates.len(),
                candidates,
                truncated,
            })
        }
    }
    fn candidate(id: &str, time: i64) -> Candidate {
        let evidence = EvidenceRef(id.into());
        let author = Author {
            recorded: None,
            embedded: Some("alice".into()),
            effective: Some("alice".into()),
            origin: AuthorOrigin::EmbeddedFallback,
        };
        Candidate {
            evidence: evidence.clone(),
            author: author.clone(),
            moment: Some(Moment {
                id: id.into(),
                evidence,
                author,
                created_at: time,
                text: "synthetic".into(),
                media: vec![],
                quality: ContentQuality::Parsed,
            }),
        }
    }
    fn request() -> Query {
        Query {
            authors: BTreeSet::from(["alice".into()]),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange {
                since: Some(10),
                until: Some(20),
            },
            keyword: None,
            limit: 10,
            scan_limit: 20,
        }
    }
    #[test]
    fn endpoints_sorting_and_local_coverage_do_not_depend_on_a_database() {
        let page = query(
            &mut Memory(vec![
                candidate("a", 10),
                candidate("b", 20),
                candidate("c", 21),
            ]),
            &request(),
        )
        .unwrap();
        assert_eq!(
            page.moments
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(page.coverage, Coverage::LocalCacheOnly);
        assert_eq!(page.filtered, 1);
    }
    #[test]
    fn recorded_export_policy_does_not_claim_embedded_only_authors() {
        let mut q = request();
        q.author_policy = AuthorPolicy::RecordedCompatibility;
        assert!(query(&mut Memory(vec![candidate("a", 10)]), &q)
            .unwrap()
            .moments
            .is_empty());
    }
    #[test]
    fn unreadable_and_truncated_are_not_reported_as_complete_empty_results() {
        let mut bad = candidate("bad", 10);
        bad.moment = None;
        let mut q = request();
        q.scan_limit = 1;
        let page = query(&mut Memory(vec![bad, candidate("a", 10)]), &q).unwrap();
        assert_eq!(page.unreadable, [EvidenceRef("bad".into())]);
        assert!(page.scan_truncated);
    }
    #[test]
    fn conflicts_and_result_pagination_are_independent_of_scan_completeness() {
        let mut conflict = candidate("conflict", 20);
        conflict.author.recorded = Some("alice".into());
        conflict.author.embedded = Some("different".into());
        conflict.author.origin = AuthorOrigin::Conflict;
        conflict.moment.as_mut().unwrap().author = conflict.author.clone();
        let mut q = request();
        q.limit = 1;
        let page = query(&mut Memory(vec![candidate("older", 10), conflict]), &q).unwrap();
        assert_eq!(page.author_conflicts, [EvidenceRef("conflict".into())]);
        assert!(page.more_matches);
        assert!(!page.scan_truncated);
        assert_eq!(page.moments[0].id, "conflict");
    }
}
