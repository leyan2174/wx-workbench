//! Voice-directory selection, not proof of a strict message association.
use super::media::{Error, Failure, Stage};

/// Portable metadata, never an authorization to read media. Coordinates are evidence only.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ManifestItem<E> {
    pub account_id: String,
    pub message_id: Option<String>,
    pub conversation: Option<String>,
    pub sender: Option<String>,
    pub timestamp: Option<i64>,
    pub duration_ms: Option<u64>,
    pub encoding: Option<String>,
    pub relative_path: Option<String>,
    pub status: String,
    pub association: String,
    pub evidence: E,
    pub failure: Option<String>,
}

/// Only a nonzero server identity can survive message database reorganization.
pub fn stable_message_id(username: &str, server_id: Option<i64>) -> Option<String> {
    server_id.filter(|id| *id != 0).map(|id| {
        serde_json::to_string(&(username, id.to_string())).expect("string tuple serialization")
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub slot: usize,
    pub username: String,
    pub timestamp: Option<i64>,
    pub local_id: Option<i64>,
}

pub trait Source {
    fn entries(&self) -> &[Entry];
}

#[derive(Default)]
pub struct Selection<'a> {
    pub username: Option<&'a str>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub offset: usize,
    pub limit: Option<usize>,
}

pub fn select(source: &impl Source, query: &Selection<'_>) -> Vec<Entry> {
    let mut entries: Vec<_> = source
        .entries()
        .iter()
        .filter(|entry| {
            query.username.is_none_or(|name| name == entry.username)
                && query
                    .since
                    .is_none_or(|time| entry.timestamp.is_some_and(|stamp| stamp >= time))
                && query
                    .until
                    .is_none_or(|time| entry.timestamp.is_some_and(|stamp| stamp <= time))
        })
        .cloned()
        .collect();
    // Stable ties retain the adapter's sorted shard/row order. Pagination is global.
    entries.sort_by_key(|entry| (entry.timestamp, entry.local_id));
    entries
        .into_iter()
        .skip(query.offset)
        .take(query.limit.unwrap_or(usize::MAX))
        .collect()
}

pub fn resolve_chat<'a>(
    chat: &str,
    names: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<String, Error> {
    let names: Vec<_> = names.into_iter().collect();
    if names.iter().any(|(username, _)| *username == chat) {
        return Ok(chat.into());
    }
    let needle = chat.to_lowercase();
    let matches: Vec<_> = names
        .into_iter()
        .filter(|(username, display)| {
            username.to_lowercase().contains(&needle) || display.to_lowercase().contains(&needle)
        })
        .collect();
    match matches.as_slice() {
        [(username, _)] => Ok((*username).into()),
        [] => Err(Error::new(Stage::Discovery, Failure::NotFound)),
        _ => Err(Error::new(Stage::Discovery, Failure::Ambiguous)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_identity_requires_server_evidence_and_conversation() {
        assert_eq!(stable_message_id("a", None), None);
        assert_eq!(stable_message_id("a", Some(0)), None);
        assert_ne!(
            stable_message_id("a", Some(1)),
            stable_message_id("b", Some(1))
        );
    }
    struct Memory(Vec<Entry>);
    impl Source for Memory {
        fn entries(&self) -> &[Entry] {
            &self.0
        }
    }
    #[test]
    fn global_page_includes_both_time_endpoints_and_stable_ties() {
        let source = Memory(
            [30, 10, 20, 20]
                .into_iter()
                .enumerate()
                .map(|(slot, timestamp)| Entry {
                    slot,
                    username: "chat".into(),
                    timestamp: Some(timestamp),
                    local_id: Some(1),
                })
                .collect(),
        );
        let query = Selection {
            since: Some(10),
            until: Some(30),
            offset: 1,
            limit: Some(2),
            ..Default::default()
        };
        assert_eq!(
            select(&source, &query)
                .iter()
                .map(|e| e.slot)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            select(
                &source,
                &Selection {
                    limit: Some(0),
                    ..Default::default()
                }
            )
            .len(),
            0
        );
        assert!(select(
            &source,
            &Selection {
                offset: usize::MAX,
                ..Default::default()
            }
        )
        .is_empty());
        assert_eq!(
            select(
                &source,
                &Selection {
                    since: Some(30),
                    until: Some(30),
                    ..Default::default()
                }
            )[0]
            .slot,
            0
        );
        assert!(select(
            &source,
            &Selection {
                username: Some("other"),
                ..Default::default()
            }
        )
        .is_empty());
    }
    #[test]
    fn legacy_resolution_keeps_exact_priority_and_casefolded_substrings() {
        let names = [("alice", "Other"), ("bob", "ALICE display")];
        assert_eq!(resolve_chat("alice", names).unwrap(), "alice");
        assert_eq!(resolve_chat("DISPLAY", names).unwrap(), "bob");
        assert_eq!(
            resolve_chat("ali", names).unwrap_err(),
            Error::new(Stage::Discovery, Failure::Ambiguous)
        );
        assert!(resolve_chat("missing", names).is_err());
    }
}
