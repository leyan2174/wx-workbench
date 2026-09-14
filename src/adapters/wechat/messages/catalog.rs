//! Account source catalog from the same validated inventory used by message reads.
pub use super::directory_selection::{select_targets, RawDirectoryTarget};
use super::Snapshot;
use crate::business::messages::{Conversation, Error, SourceKind};
use anyhow::{ensure, Result};
use std::collections::{BTreeMap, BTreeSet};

pub struct Entry {
    pub conversation: Conversation,
    pub table_name: String,
    pub sources: Vec<String>,
}

pub fn read(snapshot: &Snapshot, kind: SourceKind) -> Result<Vec<Entry>> {
    let mut entries = BTreeMap::<String, (Conversation, BTreeSet<String>)>::new();
    for (index, stream) in snapshot.streams().iter().enumerate() {
        if snapshot.source_kind(index)? != kind {
            continue;
        }
        let table = format!("Msg_{}", stream.table_name()[4..].to_ascii_lowercase());
        let (conversation, sources) = entries
            .entry(table)
            .or_insert_with(|| (stream.conversation.clone(), BTreeSet::new()));
        ensure!(*conversation == stream.conversation, Error::Ambiguous);
        sources.insert(snapshot.source_name(index)?.replace('\\', "/"));
    }
    Ok(entries
        .into_iter()
        .map(|(table_name, (conversation, sources))| Entry {
            conversation,
            table_name,
            sources: sources.into_iter().collect(),
        })
        .collect())
}
