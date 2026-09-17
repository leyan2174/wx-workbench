//! Typed ordinary reads plus an explicitly separate wire projection.
//! Reads apply ordering and per-stream candidate limits before global pagination.
use crate::adapters::wechat::messages::{display, LegacyReadPolicy, RawMessage, Snapshot};
use crate::business::messages::{self as domain, Conversation, MessageRef, SourceKind};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Not a business identity or portable locator. Only the JSON boundary
/// serializes these fields; ordinary page selection never consumes them.
#[derive(Debug, Serialize)]
pub struct LegacyMessageProjection {
    local_id: i64,
    source: String,
    #[serde(rename = "type")]
    type_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    unmapped_conversation: Option<String>,
    #[serde(skip)]
    unmapped_chat_label: Option<String>,
}

impl LegacyMessageProjection {
    /// Historical search display for an unresolved conversation, not its identity.
    pub fn unmapped_chat_label(&self) -> Option<&str> {
        self.unmapped_chat_label.as_deref()
    }
}

#[derive(Default)]
pub struct LegacyPageProjection {
    messages: HashMap<MessageRef, LegacyMessageProjection>,
}

impl LegacyPageProjection {
    pub fn message(&self, reference: &MessageRef) -> Result<&LegacyMessageProjection> {
        ensure!(!reference.evidence().is_expired(), domain::Error::Expired);
        self.messages
            .get(reference)
            .ok_or_else(|| domain::Error::InvalidData.into())
    }
}

/// Explicit diagnostic material, never part of the semantic page or its identity.
pub struct HistoryShard {
    pub logical_source: String,
    pub table: String,
    pub latest_timestamp: i64,
}

#[derive(Default)]
pub struct PageDiagnostics {
    pub hits: usize,
    pub shards: Vec<HistoryShard>,
}

pub struct ReadPage {
    pub page: domain::MessagePage,
    pub legacy: LegacyPageProjection,
    pub diagnostics: PageDiagnostics,
}

struct Decoded {
    message: domain::Message,
    legacy: LegacyMessageProjection,
}

fn selected(
    candidates: Vec<domain::Candidate<Decoded>>,
    page: &domain::Page,
    newest_first: bool,
    diagnostics: PageDiagnostics,
    candidates_exhausted: bool,
) -> Result<ReadPage> {
    let mut rows = page.select(candidates, domain::Completeness::Complete)?;
    let continuation = page.continuation(rows.len(), candidates_exhausted);
    // History/subscriptions display chronologically. Search keeps its old reverse.
    if newest_first {
        rows.reverse();
    }
    let mut messages = Vec::with_capacity(rows.len());
    let mut legacy = LegacyPageProjection::default();
    for row in rows {
        legacy
            .messages
            .insert(row.message.reference.clone(), row.legacy);
        messages.push(row.message);
    }
    Ok(ReadPage {
        page: domain::MessagePage {
            messages,
            completeness: domain::Completeness::Complete,
            continuation,
        },
        legacy,
        diagnostics,
    })
}

impl Snapshot {
    fn decoded_candidate(&self, raw: &RawMessage) -> Result<domain::Candidate<Decoded>> {
        let message = self.message(raw)?;
        let unmapped = match &message.conversation {
            Conversation::Known(_) => None,
            Conversation::Unmapped(key) => Some(key.0.clone()),
        };
        let legacy = LegacyMessageProjection {
            local_id: raw
                .local_id
                .context("ordinary message identity unavailable")?,
            source: raw.logical_source.replace('\\', "/"),
            type_label: display::fmt_type(raw.local_type),
            unmapped_chat_label: unmapped.as_ref().map(|_| {
                self.streams()[raw.reference.evidence().stream]
                    .table_name()
                    .to_owned()
            }),
            unmapped_conversation: unmapped,
        };
        Ok(domain::Candidate {
            order: self.order_key(raw)?,
            reference: message.reference.clone(),
            value: Decoded { message, legacy },
        })
    }

    pub fn history_page(
        &self,
        username: &str,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        page: &domain::Page,
    ) -> Result<ReadPage> {
        filter.validate()?;
        let per_stream = page.candidate_limit()?;
        let mut candidates = Vec::new();
        let mut diagnostics = PageDiagnostics::default();
        let mut candidates_exhausted = true;
        let mut streams = self
            .streams_for(username, SourceKind::Ordinary)
            .into_iter()
            .map(|stream| Ok((stream, self.latest_timestamp(stream)?.unwrap_or(0))))
            .collect::<Result<Vec<_>>>()?;
        streams.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (rank, (stream, latest_timestamp)) in streams.into_iter().enumerate() {
            diagnostics.shards.push(HistoryShard {
                logical_source: self.source_name(stream)?.to_owned(),
                table: self.streams()[stream].table_name().into(),
                latest_timestamp,
            });
            let rows =
                self.read_legacy_page(stream, filter, legacy, per_stream, page.oldest_first)?;
            candidates_exhausted &= rows.len() < per_stream;
            if !rows.is_empty() {
                diagnostics.hits += 1;
            }
            for raw in rows {
                ensure!(candidates.len() < 100_000, domain::Error::Limit);
                let mut candidate = self.decoded_candidate(&raw)?;
                candidate.order.1 = rank;
                candidates.push(candidate);
            }
        }
        diagnostics.shards.sort_by(|a, b| {
            b.latest_timestamp
                .cmp(&a.latest_timestamp)
                .then(a.logical_source.cmp(&b.logical_source))
        });
        selected(candidates, page, false, diagnostics, candidates_exhausted)
    }

    pub fn search_page(
        &self,
        targets: Option<&HashSet<String>>,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        keyword: &str,
        page: &domain::Page,
    ) -> Result<ReadPage> {
        filter.validate()?;
        let per_stream = page.candidate_limit()?;
        let mut candidates = Vec::new();
        let mut hits = HashSet::new();
        let mut candidates_exhausted = true;
        for (stream, entry) in self.streams().iter().enumerate() {
            if self.source_kind(stream)? != SourceKind::Ordinary {
                continue;
            }
            let username = match &entry.conversation {
                Conversation::Known(name) => name.as_str(),
                Conversation::Unmapped(_) => "",
            };
            if targets.is_some_and(|set| !set.contains(username)) {
                continue;
            }
            let rows = self.search_legacy_page(stream, filter, legacy, keyword, per_stream)?;
            candidates_exhausted &= rows.len() < per_stream;
            for raw in rows {
                ensure!(candidates.len() < 100_000, domain::Error::Limit);
                hits.insert(raw.logical_source.clone());
                candidates.push(self.decoded_candidate(&raw)?);
            }
        }
        selected(
            candidates,
            page,
            true,
            PageDiagnostics {
                hits: hits.len(),
                shards: Vec::new(),
            },
            candidates_exhausted,
        )
    }

    pub fn new_messages_page(
        &self,
        changed: &[(String, i64)],
        page: &domain::Page,
    ) -> Result<ReadPage> {
        let per_stream = page.candidate_limit()?;
        let mut candidates = Vec::new();
        let mut hits = HashSet::new();
        let mut candidates_exhausted = true;
        for (username, since) in changed {
            let filter = domain::Filter {
                since: Some(since.checked_add(1).context("subscription time overflow")?),
                until: None,
                kinds: Vec::new(),
            };
            for stream in self.streams_for(username, SourceKind::Ordinary) {
                let rows = self.read_page(stream, &filter, per_stream, true)?;
                candidates_exhausted &= rows.len() < per_stream;
                for raw in rows {
                    ensure!(candidates.len() < 100_000, domain::Error::Limit);
                    hits.insert(raw.logical_source.clone());
                    candidates.push(self.decoded_candidate(&raw)?);
                }
            }
        }
        selected(
            candidates,
            page,
            false,
            PageDiagnostics {
                hits: hits.len(),
                shards: Vec::new(),
            },
            candidates_exhausted,
        )
    }
}

#[cfg(test)]
mod tests;
