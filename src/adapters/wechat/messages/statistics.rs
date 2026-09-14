use super::{legacy, semantic_kind, Snapshot};
use crate::business::messages::{self as domain, statistics::Statistics};
use anyhow::Result;
use chrono::{Local, TimeZone, Timelike};
use std::collections::HashMap;

pub struct Report {
    pub statistics: Statistics,
    pub legacy_types: HashMap<String, u64>,
    pub hit_streams: usize,
}

pub fn read(snapshot: &Snapshot, username: &str, filter: &domain::Filter) -> Result<Report> {
    filter.validate()?;
    let mut report = Report {
        statistics: Statistics::default(),
        legacy_types: HashMap::new(),
        hit_streams: 0,
    };
    let group = username.ends_with("@chatroom");
    for stream in snapshot.streams_for(username, domain::SourceKind::Ordinary) {
        let found = snapshot.visit_metadata_counts(stream, filter, group, |row| {
            let hour = Local
                .timestamp_opt(row.timestamp, 0)
                .single()
                .ok_or(domain::Error::InvalidData)?
                .hour() as usize;
            report.statistics.add(
                semantic_kind(row.local_type),
                hour,
                row.sender.as_deref().filter(|s| *s != username),
                row.count,
            )?;
            let count = report
                .legacy_types
                .entry(legacy::fmt_type(row.local_type & 0xffff_ffff))
                .or_default();
            *count = count.checked_add(row.count).ok_or(domain::Error::Limit)?;
            Ok(())
        })?;
        if found {
            report.hit_streams += 1;
        }
    }
    Ok(report)
}
