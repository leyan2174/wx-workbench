//! Locally received article pushes in one fixed account, not remote article history.
use std::{
    collections::{BTreeSet, HashMap},
    fmt,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvidenceRef {
    message: super::messages::MessageRef,
    item_index: Option<u32>,
}
impl EvidenceRef {
    pub(crate) fn new(message: super::messages::MessageRef, item_index: Option<u32>) -> Self {
        Self {
            message,
            item_index,
        }
    }
    pub fn message(&self) -> &super::messages::MessageRef {
        &self.message
    }
    pub fn item_index(&self) -> Option<u32> {
        self.item_index
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Article {
    pub evidence: EvidenceRef,
    pub publisher: String,
    pub publisher_name: String,
    pub received_at: i64,
    pub published_at: i64,
    pub title: String,
    pub url: String,
    pub digest: String,
    pub cover_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    UnknownPublisher,
    InvalidContent,
    UnavailableSource,
    UnsupportedSource,
    AmbiguousIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub evidence: Option<EvidenceRef>,
    pub kind: IssueKind,
}

#[derive(Default)]
pub struct Inventory {
    pub articles: Vec<Article>,
    pub issues: Vec<Issue>,
    pub unfinished: bool,
}

pub struct Query {
    pub limit: usize,
    pub publisher: Option<String>,
    pub received_since: Option<i64>,
    pub received_until: Option<i64>,
    /// None means all publishers; Some(empty) means no unread publishers.
    pub unread_publishers: Option<BTreeSet<String>>,
}

pub struct Page {
    pub articles: Vec<Article>,
    pub issues: Vec<Issue>,
    pub source_unfinished: bool,
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    UnsupportedFormat,
    InvalidData,
    InvalidQuery,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "article source unavailable",
            Self::UnsupportedFormat => "article source format unsupported",
            Self::InvalidData => "article source contains invalid data",
            Self::InvalidQuery => "invalid article query",
        })
    }
}
impl std::error::Error for Error {}

/// Implementations may prefilter, but must not limit pushes before expanding articles.
pub trait ArticleSource {
    fn scan(&mut self, query: &Query) -> Result<Inventory, Error>;
}

pub fn list(source: &mut impl ArticleSource, query: &Query) -> Result<Page, Error> {
    if matches!((query.received_since, query.received_until), (Some(a), Some(b)) if a > b) {
        return Err(Error::InvalidQuery);
    }
    let mut inventory = source.scan(query)?;
    let publisher = query.publisher.as_ref().map(|value| value.to_lowercase());
    let mut evidence_counts = HashMap::new();
    for article in &inventory.articles {
        if article.evidence.message.evidence().is_expired() {
            return Err(Error::InvalidData);
        }
        *evidence_counts
            .entry(article.evidence.clone())
            .or_insert(0usize) += 1;
    }
    inventory.articles.retain(|article| {
        let issue = if article.publisher.is_empty() {
            Some(IssueKind::UnknownPublisher)
        } else if article.evidence.item_index.is_none() || evidence_counts[&article.evidence] != 1 {
            Some(IssueKind::AmbiguousIdentity)
        } else {
            None
        };
        if let Some(kind) = issue {
            inventory.issues.push(Issue {
                evidence: Some(article.evidence.clone()),
                kind,
            });
            return false;
        }
        query
            .received_since
            .is_none_or(|time| article.received_at >= time)
            && query
                .received_until
                .is_none_or(|time| article.received_at <= time)
            && publisher.as_ref().is_none_or(|text| {
                article.publisher.to_lowercase().contains(text)
                    || article.publisher_name.to_lowercase().contains(text)
            })
            && query
                .unread_publishers
                .as_ref()
                .is_none_or(|set| set.contains(&article.publisher))
    });
    inventory
        .articles
        .sort_by_key(|article| std::cmp::Reverse(article.published_at));
    if query.unread_publishers.is_some() {
        let mut publishers = BTreeSet::new();
        inventory
            .articles
            .retain(|article| publishers.insert(article.publisher.clone()));
    }
    let has_more = inventory.articles.len() > query.limit;
    inventory.articles.truncate(query.limit);
    Ok(Page {
        articles: inventory.articles,
        issues: inventory.issues,
        source_unfinished: inventory.unfinished,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(id: &str) -> EvidenceRef {
        use std::sync::{Arc, OnceLock};
        static OWNER: OnceLock<Arc<()>> = OnceLock::new();
        let owner = OWNER.get_or_init(|| Arc::new(()));
        EvidenceRef::new(
            super::super::messages::MessageRef(super::super::messages::EvidenceRef {
                snapshot: Arc::downgrade(owner),
                stream: 0,
                record: id.parse().unwrap(),
            }),
            Some(id.parse().unwrap()),
        )
    }
    struct Memory(Option<Inventory>);
    impl ArticleSource for Memory {
        fn scan(&mut self, _: &Query) -> Result<Inventory, Error> {
            Ok(self.0.take().unwrap())
        }
    }
    fn article(id: &str, publisher: &str, received: i64, published: i64) -> Article {
        Article {
            evidence: evidence(id),
            publisher: publisher.into(),
            publisher_name: format!("Name {publisher}"),
            received_at: received,
            published_at: published,
            title: "title".into(),
            url: "https://example.invalid/article".into(),
            digest: String::new(),
            cover_url: String::new(),
        }
    }
    fn query() -> Query {
        Query {
            limit: 10,
            publisher: None,
            received_since: None,
            received_until: None,
            unread_publishers: None,
        }
    }
    fn source(articles: Vec<Article>) -> Memory {
        Memory(Some(Inventory {
            articles,
            ..Inventory::default()
        }))
    }
    #[test]
    fn receive_endpoints_and_publication_order_are_distinct() {
        let mut source = source(vec![
            article("1", "a", 10, 100),
            article("2", "b", 20, 1),
            article("3", "c", 21, 200),
        ]);
        let q = Query {
            received_since: Some(10),
            received_until: Some(20),
            ..query()
        };
        let page = list(&mut source, &q).unwrap();
        assert_eq!(
            page.articles
                .iter()
                .map(|a| a.evidence.item_index().unwrap())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }
    #[test]
    fn unread_is_publisher_intersection_then_latest_one_not_read_state_of_article() {
        let mut source = source(vec![
            article("1", "a", 10, 1),
            article("2", "a", 10, 2),
            article("3", "b", 10, 3),
        ]);
        let q = Query {
            publisher: Some("NAME A".into()),
            unread_publishers: Some(BTreeSet::from(["a".into(), "b".into()])),
            ..query()
        };
        let page = list(&mut source, &q).unwrap();
        assert_eq!(page.articles.len(), 1);
        assert_eq!(page.articles[0].evidence.item_index(), Some(2));
    }
    #[test]
    fn unknown_and_duplicate_identity_are_explicit_not_empty_publishers() {
        let mut source = source(vec![
            article("1", "", 1, 1),
            article("2", "a", 1, 1),
            article("2", "b", 1, 1),
        ]);
        let page = list(&mut source, &query()).unwrap();
        assert!(page.articles.is_empty());
        assert_eq!(
            page.issues.iter().map(|i| i.kind).collect::<Vec<_>>(),
            [
                IssueKind::UnknownPublisher,
                IssueKind::AmbiguousIdentity,
                IssueKind::AmbiguousIdentity
            ]
        );
    }
    #[test]
    fn pagination_does_not_hide_partial_source_or_collapse_same_url() {
        let mut source = source(vec![article("1", "a", 1, 1), article("2", "a", 1, 1)]);
        source.0.as_mut().unwrap().unfinished = true;
        let page = list(
            &mut source,
            &Query {
                limit: 1,
                ..query()
            },
        )
        .unwrap();
        assert!(page.has_more && page.source_unfinished);
        assert_eq!(page.articles[0].evidence.item_index(), Some(1));
    }
}
