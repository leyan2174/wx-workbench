//! Voice-directory selection, not proof of a strict message association.
use super::media::{Error, Failure, Stage};

/// Explicit batch selection; this is not a strict message identity.
pub fn batch_selected(
    username: &str,
    contacts: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    contacts.is_none_or(|names| names.is_empty() || names.contains(username))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchItemOutcome {
    Converted,
    Existing,
    Filtered,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchState {
    Success,
    Partial,
    Failure,
}

#[derive(Debug, Clone, Copy)]
pub enum BatchFailureStage {
    Metadata,
    Directory,
    Material,
    Conversion,
    Publication,
}
#[derive(Debug, Default, serde::Serialize)]
pub struct BatchProgress {
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub converted: u64,
    pub skipped_existing: u64,
    pub filtered: u64,
}

impl BatchProgress {
    pub fn record(&mut self, outcome: BatchItemOutcome) {
        self.total += 1;
        match outcome {
            BatchItemOutcome::Converted => {
                self.converted += 1;
                self.success += 1;
            }
            BatchItemOutcome::Existing => {
                self.skipped_existing += 1;
                self.success += 1;
            }
            BatchItemOutcome::Filtered => self.filtered += 1,
            BatchItemOutcome::Failed => self.failed += 1,
        }
    }

    pub fn state(&self) -> BatchState {
        if self.failed == 0 {
            BatchState::Success
        } else if self.converted + self.skipped_existing > 0 {
            BatchState::Partial
        } else {
            BatchState::Failure
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub slot: usize,
    pub username: String,
    pub timestamp: i64,
    pub local_id: i64,
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
                && query.since.is_none_or(|time| entry.timestamp >= time)
                && query.until.is_none_or(|time| entry.timestamp <= time)
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
    fn batch_counts_and_exact_selection_preserve_legacy_success() {
        let mut report = BatchProgress::default();
        report.record(BatchItemOutcome::Filtered);
        assert_eq!(report.state(), BatchState::Success);
        report.record(BatchItemOutcome::Failed);
        assert_eq!(report.state(), BatchState::Failure);
        report.record(BatchItemOutcome::Existing);
        assert_eq!(report.state(), BatchState::Partial);
        report.record(BatchItemOutcome::Converted);
        assert_eq!(
            (report.total, report.success, report.failed, report.filtered),
            (4, 2, 1, 1)
        );
        let names = ["Alice".to_owned()].into_iter().collect();
        assert!(batch_selected("Alice", Some(&names)));
        assert!(!batch_selected("alice", Some(&names)));
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
                    timestamp,
                    local_id: 1,
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
