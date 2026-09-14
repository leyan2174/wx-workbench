//! Favorites in one fixed account. Physical identity remains opaque to callers.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteId(pub String);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceRef(pub(crate) String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FavoriteKind {
    Text,
    Image,
    Article,
    ContactCard,
    Video,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub id: FavoriteId,
    pub evidence: EvidenceRef,
    pub kind: FavoriteKind,
    pub updated_at: i64,
    pub text: Option<String>,
    pub author: Option<String>,
    pub conversation: Option<String>,
    pub article_url: Option<String>,
}

pub struct Query {
    pub limit: usize,
    pub text: Option<String>,
}
pub struct Page {
    pub items: Vec<Favorite>,
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    UnsupportedFormat,
    InvalidData,
    InvalidQuery,
    Busy,
    Cancelled,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "favorite source unavailable",
            Self::UnsupportedFormat => "favorite source format unsupported",
            Self::InvalidData => "favorite source contains invalid data",
            Self::InvalidQuery => "invalid favorite query",
            Self::Busy => "favorite source busy",
            Self::Cancelled => "favorite query cancelled",
        })
    }
}
impl std::error::Error for Error {}

/// A source is bound to a single account and orders results newest first.
pub trait FavoriteSource {
    fn scan(&mut self, query: &Query) -> Result<Page, Error>;
}

pub fn list(source: &mut impl FavoriteSource, query: &Query) -> Result<Page, Error> {
    if query.limit >= i64::MAX as usize {
        return Err(Error::InvalidQuery);
    }
    source.scan(query)
}

pub fn preview(content: &str, limit: usize) -> String {
    let mut chars = content.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemorySource;
    impl FavoriteSource for MemorySource {
        fn scan(&mut self, query: &Query) -> Result<Page, Error> {
            assert_eq!(query.text.as_deref(), Some("needle"));
            Ok(Page {
                items: vec![],
                has_more: true,
            })
        }
    }

    #[test]
    fn use_case_retains_source_completeness_and_validates_before_reading() {
        assert!(
            list(
                &mut MemorySource,
                &Query {
                    limit: 2,
                    text: Some("needle".into())
                }
            )
            .unwrap()
            .has_more
        );
        assert!(matches!(
            list(
                &mut MemorySource,
                &Query {
                    limit: usize::MAX,
                    text: None
                }
            ),
            Err(Error::InvalidQuery)
        ));
    }

    #[test]
    fn preview_is_unicode_safe_and_marks_only_actual_truncation() {
        assert_eq!(preview("a\u{1f600}b", 2), "a\u{1f600}...");
        assert_eq!(preview("ab", 2), "ab");
        assert_eq!(preview("", 2), "");
    }
}
