//! Export planning rules over supplied source contributions. No filesystem or SQL.
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct PlanChat {
    pub index: usize,
    pub username: String,
    pub chat_name: String,
    pub chat_type: String,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeRange {
    pub start: Option<i64>,
    pub end: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeMode {
    Estimate,
    Scan,
}
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct MessageStatistics {
    pub message_count: i64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub message_body_bytes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Partial {
    MediaReadFailed,
    MediaSourceMissing,
    MessageSourceMissing,
    MessageReadFailed,
    ConversationAbsent,
    ResourceReadFailed,
    ResourceSourceMissing,
    ScanBaseMissing,
    ScanDepthLimited,
    ScanError,
    ScanLimited,
    ScanMissing,
    ScanOverflow,
    ScanReparseSkipped,
}
impl std::fmt::Display for Partial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Overflow,
    InvalidContribution,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plan {:?}", self)
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Clone, Copy)]
pub struct ReadFailure;
pub type Read<T> = Result<T, ReadFailure>;

/// Ordered source contributions are intentional: media aggregation stops at first error.
/// Source indexes are positions in this read's supplied sequence, never storage identifiers.
pub trait Source {
    fn message_source_count(&self) -> usize;
    fn media_source_count(&self) -> usize;
    fn message_contribution(
        &mut self,
        source_index: usize,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> Read<Vec<Read<Option<MessageStatistics>>>>;
    fn resource_contribution(
        &mut self,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> Read<Option<Vec<i64>>>;
    fn media_contribution(
        &mut self,
        source_index: usize,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> Read<Vec<i64>>;
}

#[derive(Default)]
pub struct ScanContribution {
    pub bytes: i64,
    pub statuses: BTreeSet<Partial>,
}
pub struct PlannedChat {
    pub chat: PlanChat,
    pub messages: MessageStatistics,
    pub attachment_estimated_bytes: i64,
    pub attachment_scanned_bytes: Option<i64>,
    pub total_estimated_bytes: i64,
    pub statuses: BTreeSet<Partial>,
}
impl PlannedChat {
    pub fn apply_scan(&mut self, scan: ScanContribution) {
        self.attachment_scanned_bytes = Some(scan.bytes);
        self.statuses.extend(scan.statuses);
    }
}
pub struct Plan {
    pub rows: Vec<PlannedChat>,
}
pub fn add(target: &mut i64, value: i64) -> Result<(), Error> {
    *target = target.checked_add(value).ok_or(Error::Overflow)?;
    Ok(())
}
pub fn validate_range(range: TimeRange) -> Result<(), Error> {
    if matches!((range.start, range.end), (Some(a), Some(b)) if a > b) {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
impl Plan {
    pub fn read(
        source: &mut impl Source,
        chats: &[PlanChat],
        range: TimeRange,
    ) -> Result<Self, Error> {
        validate_range(range)?;
        let mut names = BTreeSet::new();
        if chats
            .iter()
            .any(|c| c.username.is_empty() || !names.insert(&c.username))
        {
            return Err(Error::InvalidInput);
        }
        let mut rows: Vec<_> = chats
            .iter()
            .cloned()
            .map(|chat| PlannedChat {
                chat,
                messages: MessageStatistics::default(),
                attachment_estimated_bytes: 0,
                attachment_scanned_bytes: None,
                total_estimated_bytes: 0,
                statuses: BTreeSet::new(),
            })
            .collect();
        let mut found = vec![false; rows.len()];
        for source_index in 0..source.message_source_count() {
            let values = match source.message_contribution(source_index, chats, range) {
                Ok(values) => values,
                Err(_) => {
                    for row in &mut rows {
                        row.statuses.insert(Partial::MessageReadFailed);
                    }
                    continue;
                }
            };
            if values.len() != rows.len() {
                return Err(Error::InvalidContribution);
            }
            for (i, value) in values.into_iter().enumerate() {
                let row = &mut rows[i];
                match value {
                    Ok(Some(value)) => {
                        found[i] = true;
                        add(&mut row.messages.message_count, value.message_count)?;
                        add(
                            &mut row.messages.message_body_bytes,
                            value.message_body_bytes,
                        )?;
                        if let Some(t) = value.first_ts.filter(|t| *t != 0) {
                            row.messages.first_ts =
                                Some(row.messages.first_ts.map_or(t, |old| old.min(t)));
                        }
                        if let Some(t) = value.last_ts.filter(|t| *t != 0) {
                            row.messages.last_ts =
                                Some(row.messages.last_ts.map_or(t, |old| old.max(t)));
                        }
                    }
                    Ok(None) => (),
                    Err(_) => {
                        row.statuses.insert(Partial::MessageReadFailed);
                    }
                }
            }
        }
        for (i, row) in rows.iter_mut().enumerate() {
            if source.message_source_count() == 0 {
                row.statuses.insert(Partial::MessageSourceMissing);
            } else if !found[i] {
                row.statuses.insert(Partial::ConversationAbsent);
            }
        }
        match source.resource_contribution(chats, range) {
            Ok(Some(values)) => {
                if values.len() != rows.len() {
                    return Err(Error::InvalidContribution);
                }
                for (row, value) in rows.iter_mut().zip(values) {
                    row.attachment_estimated_bytes = value;
                }
            }
            result => {
                let status = if result.is_ok() {
                    Partial::ResourceSourceMissing
                } else {
                    Partial::ResourceReadFailed
                };
                for row in &mut rows {
                    row.statuses.insert(status);
                }
            }
        }
        if source.media_source_count() == 0 {
            for row in &mut rows {
                row.statuses.insert(Partial::MediaSourceMissing);
            }
        }
        for source_index in 0..source.media_source_count() {
            match source.media_contribution(source_index, chats, range) {
                Ok(values) => {
                    if values.len() != rows.len() {
                        return Err(Error::InvalidContribution);
                    }
                    for (row, value) in rows.iter_mut().zip(values) {
                        add(&mut row.attachment_estimated_bytes, value)?;
                    }
                }
                Err(_) => {
                    for row in &mut rows {
                        row.statuses.insert(Partial::MediaReadFailed);
                    }
                    break;
                }
            }
        }
        for row in &mut rows {
            row.total_estimated_bytes = row.messages.message_body_bytes;
            add(
                &mut row.total_estimated_bytes,
                row.attachment_estimated_bytes,
            )?;
        }
        Ok(Self { rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory {
        messages: Vec<Read<Vec<Read<Option<MessageStatistics>>>>>,
        resources: Read<Option<Vec<i64>>>,
        media: Vec<Read<Vec<i64>>>,
        media_reads: Vec<usize>,
    }
    impl Source for Memory {
        fn message_source_count(&self) -> usize {
            self.messages.len()
        }
        fn media_source_count(&self) -> usize {
            self.media.len()
        }
        fn message_contribution(
            &mut self,
            source_index: usize,
            _: &[PlanChat],
            _: TimeRange,
        ) -> Read<Vec<Read<Option<MessageStatistics>>>> {
            self.messages[source_index].clone()
        }
        fn resource_contribution(
            &mut self,
            _: &[PlanChat],
            _: TimeRange,
        ) -> Read<Option<Vec<i64>>> {
            self.resources.clone()
        }
        fn media_contribution(
            &mut self,
            source_index: usize,
            _: &[PlanChat],
            _: TimeRange,
        ) -> Read<Vec<i64>> {
            self.media_reads.push(source_index);
            self.media[source_index].clone()
        }
    }
    fn chats() -> Vec<PlanChat> {
        vec![PlanChat {
            index: 9,
            username: "u".into(),
            chat_name: "n".into(),
            chat_type: "single".into(),
        }]
    }
    fn empty() -> Memory {
        Memory {
            messages: vec![],
            resources: Ok(None),
            media: vec![],
            media_reads: vec![],
        }
    }
    #[test]
    fn contributions_keep_completed_sources_and_stop_at_first_media_failure() {
        let mut source = empty();
        source.messages = vec![
            Ok(vec![Ok(Some(MessageStatistics {
                message_count: 2,
                first_ts: Some(0),
                last_ts: Some(10),
                message_body_bytes: 7,
            }))]),
            Err(ReadFailure),
        ];
        source.resources = Ok(Some(vec![5]));
        source.media = vec![Ok(vec![3]), Err(ReadFailure), Ok(vec![99])];
        let mut plan = Plan::read(&mut source, &chats(), TimeRange::default()).unwrap();
        assert_eq!(source.media_reads, vec![0, 1]);
        let row = &mut plan.rows[0];
        assert_eq!(row.chat.index, 9);
        assert_eq!(row.messages.first_ts, None);
        assert_eq!(row.messages.last_ts, Some(10));
        assert_eq!(row.total_estimated_bytes, 15);
        assert_eq!(
            row.statuses,
            BTreeSet::from([Partial::MessageReadFailed, Partial::MediaReadFailed])
        );
        row.apply_scan(ScanContribution {
            bytes: 1000,
            statuses: BTreeSet::from([Partial::ScanLimited]),
        });
        assert_eq!(row.total_estimated_bytes, 15);
        assert_eq!(row.attachment_scanned_bytes, Some(1000));
        assert!(row.statuses.contains(&Partial::ScanLimited));
    }
    #[test]
    fn absence_empty_table_and_read_failure_are_distinct() {
        let mut source = empty();
        let row = Plan::read(&mut source, &chats(), TimeRange::default())
            .unwrap()
            .rows
            .remove(0);
        assert!(row.statuses.contains(&Partial::MessageSourceMissing));
        assert!(row.statuses.contains(&Partial::ResourceSourceMissing));
        source.messages = vec![Ok(vec![Ok(None)])];
        assert!(Plan::read(&mut source, &chats(), TimeRange::default())
            .unwrap()
            .rows[0]
            .statuses
            .contains(&Partial::ConversationAbsent));
        source.messages = vec![Ok(vec![Ok(Some(MessageStatistics::default()))])];
        assert!(!Plan::read(&mut source, &chats(), TimeRange::default())
            .unwrap()
            .rows[0]
            .statuses
            .contains(&Partial::ConversationAbsent));
        source.messages = vec![Err(ReadFailure)];
        let plan = Plan::read(&mut source, &chats(), TimeRange::default()).unwrap();
        assert!(plan.rows[0].statuses.contains(&Partial::MessageReadFailed));
        assert!(plan.rows[0].statuses.contains(&Partial::ConversationAbsent));
    }
    #[test]
    fn invalid_contributions_requests_and_overflow_fail() {
        let mut source = empty();
        source.resources = Ok(Some(vec![]));
        assert!(matches!(
            Plan::read(&mut source, &chats(), TimeRange::default()),
            Err(Error::InvalidContribution)
        ));
        source.resources = Ok(Some(vec![i64::MAX]));
        source.media = vec![Ok(vec![1])];
        assert!(matches!(
            Plan::read(&mut source, &chats(), TimeRange::default()),
            Err(Error::Overflow)
        ));
        assert!(matches!(
            Plan::read(
                &mut source,
                &chats(),
                TimeRange {
                    start: Some(2),
                    end: Some(1)
                }
            ),
            Err(Error::InvalidInput)
        ));
        let mut duplicate = chats();
        duplicate.extend(chats());
        assert!(matches!(
            Plan::read(&mut source, &duplicate, TimeRange::default()),
            Err(Error::InvalidInput)
        ));
    }
}
