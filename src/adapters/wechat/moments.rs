//! SQLite/XML interpretation for account-bound local moments, not remote history.
pub mod cache;
pub mod decode;
pub mod legacy;
pub(crate) mod query_xml;

#[cfg(test)]
use crate::business::moments as business;
use crate::business::moments::*;
use rusqlite::{types::ValueRef, Connection};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

pub const MAX_QUERY_SCAN: usize = 50_000;
pub const MAX_QUERY_LIMIT: usize = 50_000;

pub const fn source_key() -> &'static str {
    "sns/sns.db"
}

pub async fn database_path(db: &crate::daemon::cache::DbCache) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    db.get(source_key()).await?.context("无法解密 sns.db")
}

/// Recorded ownership always wins a conflict. Embedded identity is a fallback only.
pub fn author(recorded: &str, embedded: &str) -> Author {
    let recorded = (!recorded.is_empty()).then(|| recorded.to_owned());
    let embedded = (!embedded.is_empty()).then(|| embedded.to_owned());
    let origin = match (&recorded, &embedded) {
        (Some(a), Some(b)) if a != b => AuthorOrigin::Conflict,
        (Some(_), _) => AuthorOrigin::Recorded,
        (None, Some(_)) => AuthorOrigin::EmbeddedFallback,
        _ => AuthorOrigin::Unknown,
    };
    Author {
        effective: recorded.clone().or_else(|| embedded.clone()),
        recorded,
        embedded,
        origin,
    }
}

fn reference(id: i64) -> EvidenceRef {
    EvidenceRef(format!("moment:{id}"))
}

/// Explicit compatibility projection; business code never branches on this locator.
pub fn legacy_record_id(reference: &EvidenceRef) -> Result<i64, SourceError> {
    reference
        .0
        .strip_prefix("moment:")
        .and_then(|s| s.parse().ok())
        .ok_or(SourceError::InvalidData)
}

#[derive(Clone, Copy)]
pub enum ReadPolicy {
    /// Preserve daemon text recovery and literal encoded-content search.
    QueryCompatibility,
    /// Preserve strict bounded export parsing, including binary/base64 content.
    ExportCompatibility(legacy::TimeZone),
}

enum Projection {
    Query(query_xml::ParsedPost),
    Export(legacy::Post),
}

pub struct Timeline<'a> {
    connection: &'a Connection,
    policy: ReadPolicy,
    projections: HashMap<EvidenceRef, Projection>,
    row_numbers: HashMap<EvidenceRef, usize>,
}

impl<'a> Timeline<'a> {
    pub fn new(connection: &'a Connection, policy: ReadPolicy) -> Self {
        Self {
            connection,
            policy,
            projections: HashMap::new(),
            row_numbers: HashMap::new(),
        }
    }
    pub(crate) fn query_projection(
        &self,
        moment: &Moment,
    ) -> Result<query_xml::ParsedPost, SourceError> {
        match self.projections.get(&moment.evidence) {
            Some(Projection::Query(post)) => Ok(post.clone()),
            _ => Err(SourceError::InvalidData),
        }
    }
    pub fn export_projection(&self, moment: &Moment) -> Result<legacy::Post, SourceError> {
        match self.projections.get(&moment.evidence) {
            Some(Projection::Export(post)) => Ok(post.clone()),
            _ => Err(SourceError::InvalidData),
        }
    }
    pub fn row_number(&self, evidence: &EvidenceRef) -> Result<usize, SourceError> {
        self.row_numbers
            .get(evidence)
            .copied()
            .ok_or(SourceError::InvalidData)
    }
}

fn columns(connection: &Connection, table: &str, required: &[&str]) -> Result<(), SourceError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|_| SourceError::Unavailable)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| SourceError::Unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| SourceError::InvalidData)?;
    if required
        .iter()
        .all(|name| names.iter().any(|found| found.eq_ignore_ascii_case(name)))
    {
        Ok(())
    } else {
        Err(SourceError::UnsupportedFormat)
    }
}

impl TimelineSource for Timeline<'_> {
    fn scan(&mut self, query: &Query) -> Result<Scan, SourceError> {
        if matches!(self.policy, ReadPolicy::ExportCompatibility(_)) && query.keyword.is_some() {
            return Err(SourceError::InvalidQuery);
        }
        let keyword = query.keyword.as_deref();
        let limit = query.scan_limit;
        self.projections.clear();
        self.row_numbers.clear();
        columns(
            self.connection,
            "SnsTimeLine",
            &["tid", "user_name", "content"],
        )?;
        let mut sql = "SELECT tid, user_name, content FROM SnsTimeLine".to_owned();
        let pattern = keyword.map(|s| format!("%{}%", query_xml::escape_like_pattern(s)));
        match self.policy {
            ReadPolicy::QueryCompatibility => {
                if pattern.is_some() {
                    sql.push_str(" WHERE content LIKE ?1 ESCAPE '\\'");
                }
                sql.push_str(" ORDER BY tid DESC");
            }
            ReadPolicy::ExportCompatibility(_) => sql.push_str(" WHERE content IS NOT NULL"),
        }
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| SourceError::UnsupportedFormat)?;
        let mut rows =
            if matches!(self.policy, ReadPolicy::QueryCompatibility) && pattern.is_some() {
                statement.query([pattern.as_deref().unwrap()])
            } else {
                statement.query([])
            }
            .map_err(|_| SourceError::Unavailable)?;
        let mut candidates = Vec::new();
        let mut scanned = 0;
        let mut truncated = false;
        while let Some(row) = rows.next().map_err(|_| SourceError::InvalidData)? {
            if scanned == limit {
                truncated = true;
                break;
            }
            scanned += 1;
            let recorded = match self.policy {
                ReadPolicy::QueryCompatibility => row.get::<_, String>(1).unwrap_or_default(),
                ReadPolicy::ExportCompatibility(_) => row
                    .get::<_, Option<String>>(1)
                    .map_err(|_| SourceError::InvalidData)?
                    .unwrap_or_default(),
            };
            let recorded_author = author(&recorded, "");
            if matches!(self.policy, ReadPolicy::ExportCompatibility(_))
                && query.author_policy == AuthorPolicy::RecordedCompatibility
                && !query.matches_author(&recorded_author)
            {
                candidates.push(Candidate {
                    evidence: EvidenceRef(format!("filtered:{scanned}")),
                    author: recorded_author,
                    moment: None,
                });
                continue;
            }
            let id: i64 = row.get(0).map_err(|_| SourceError::InvalidData)?;
            let evidence = reference(id);
            if self.row_numbers.insert(evidence.clone(), scanned).is_some() {
                return Err(SourceError::AmbiguousIdentity);
            }
            let projection = match self.policy {
                ReadPolicy::QueryCompatibility => {
                    let text = row.get::<_, String>(2).unwrap_or_default();
                    // Keep the prior literal encoded-body search, distinct from decoded display.
                    if keyword.is_some_and(|needle| {
                        !query_xml::extract_xml_text(&text, "contentDesc")
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&needle.to_lowercase())
                    }) {
                        continue;
                    }
                    Some(Projection::Query(query_xml::parse_post_xml(
                        id, &recorded, &text,
                    )))
                }
                ReadPolicy::ExportCompatibility(zone) => {
                    let parsed = match row.get_ref(2).map_err(|_| SourceError::InvalidData)? {
                        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().and_then(|s| {
                            legacy::parse_timeline(decode::Content::Text(s), zone)
                                .ok()
                                .flatten()
                        }),
                        ValueRef::Blob(bytes) => {
                            legacy::parse_timeline(decode::Content::Blob(bytes), zone)
                                .ok()
                                .flatten()
                        }
                        _ => None,
                    };
                    parsed.map(|mut post| {
                        post.tid = Some(id);
                        post.db_user_name = Some(recorded.clone());
                        Projection::Export(post)
                    })
                }
            };
            let Some(projection) = projection else {
                candidates.push(Candidate {
                    evidence,
                    author: author(&recorded, ""),
                    moment: None,
                });
                continue;
            };
            let (identity, ownership, created_at, text, media_ids, quality) = match &projection {
                Projection::Query(post) => (
                    post.post_id.clone(),
                    post.author.clone(),
                    post.create_time,
                    post.content.clone(),
                    post.media
                        .iter()
                        .map(|media| {
                            media
                                .get("id")
                                .and_then(|id| id.as_str())
                                .map(str::to_owned)
                        })
                        .collect::<Vec<_>>(),
                    if post.recovered {
                        ContentQuality::RecoveredText
                    } else {
                        ContentQuality::Parsed
                    },
                ),
                Projection::Export(post) => (
                    post.id.clone(),
                    author(&recorded, &post.username),
                    post.create_time,
                    post.content_desc.clone(),
                    post.cache_media_ids
                        .iter()
                        .map(|s| (!s.is_empty()).then(|| s.clone()))
                        .collect(),
                    ContentQuality::Parsed,
                ),
            };
            let moment = Moment {
                id: if identity.is_empty() {
                    evidence.0.clone()
                } else {
                    identity
                },
                evidence: evidence.clone(),
                author: ownership.clone(),
                created_at,
                text,
                quality,
                media: media_ids
                    .into_iter()
                    .enumerate()
                    .map(|(index, identity)| MediaRef {
                        evidence: EvidenceRef(format!("{}:media:{index}", evidence.0)),
                        identity,
                    })
                    .collect(),
            };
            if self
                .projections
                .insert(evidence.clone(), projection)
                .is_some()
            {
                return Err(SourceError::AmbiguousIdentity);
            }
            candidates.push(Candidate {
                evidence,
                author: ownership,
                moment: Some(moment),
            });
        }
        Ok(Scan {
            candidates,
            scanned,
            truncated,
        })
    }
}

pub struct Notifications<'a>(pub &'a Connection);
impl InteractionSource for Notifications<'_> {
    fn interactions(&mut self, query: &InteractionQuery) -> Result<Vec<Interaction>, SourceError> {
        columns(
            self.0,
            "SnsMessage_tmp3",
            &[
                "local_id",
                "create_time",
                "feed_id",
                "from_username",
                "from_nickname",
                "content",
                "is_unread",
            ],
        )?;
        let mut statement = self.0.prepare("SELECT local_id, create_time, feed_id, from_username, from_nickname, content, is_unread FROM SnsMessage_tmp3 WHERE (?1 OR is_unread=1) AND (?2 IS NULL OR create_time>=?2) AND (?3 IS NULL OR create_time<=?3) ORDER BY create_time DESC LIMIT ?4").map_err(|_| SourceError::UnsupportedFormat)?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    query.include_read,
                    query.time.since,
                    query.time.until,
                    query.limit.min(i64::MAX as usize) as i64
                ],
                |row| {
                    let text: String = row.get::<_, String>(5).unwrap_or_default();
                    Ok(Interaction {
                        evidence: EvidenceRef(format!("interaction:{}", row.get::<_, i64>(0)?)),
                        moment: reference(row.get::<_, i64>(2).unwrap_or(0)),
                        created_at: row.get(1)?,
                        actor: row.get::<_, String>(3).unwrap_or_default(),
                        actor_name: row.get::<_, String>(4).unwrap_or_default(),
                        kind: if text.trim().is_empty() {
                            InteractionKind::Like
                        } else {
                            InteractionKind::Comment
                        },
                        text,
                        unread: row.get::<_, i64>(6).unwrap_or(0) == 1,
                        original_author: None,
                        original_preview: None,
                    })
                },
            )
            .map_err(|_| SourceError::Unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| SourceError::InvalidData)?;
        let mut rows = rows;
        if rows.is_empty() {
            return Ok(rows);
        }
        columns(self.0, "SnsTimeLine", &["tid", "user_name", "content"])?;
        let mut feeds = HashMap::new();
        // Bound placeholders and keep a missing original distinct from an empty preview.
        let ids = rows
            .iter()
            .map(|r| legacy_record_id(&r.moment))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        for chunk in ids.into_iter().collect::<Vec<_>>().chunks(900) {
            let sql = format!(
                "SELECT tid,user_name,content FROM SnsTimeLine WHERE tid IN ({})",
                vec!["?"; chunk.len()].join(",")
            );
            let mut statement = self
                .0
                .prepare(&sql)
                .map_err(|_| SourceError::UnsupportedFormat)?;
            let mut feed_rows = statement
                .query(rusqlite::params_from_iter(chunk))
                .map_err(|_| SourceError::Unavailable)?;
            while let Some(row) = feed_rows.next().map_err(|_| SourceError::InvalidData)? {
                let id: i64 = row.get(0).map_err(|_| SourceError::InvalidData)?;
                let recorded = row.get::<_, String>(1).unwrap_or_default();
                let text = row.get::<_, String>(2).unwrap_or_default();
                let embedded = query_xml::extract_xml_text(&text, "username").unwrap_or_default();
                let ownership = author(&recorded, &embedded);
                let preview = query_xml::extract_xml_text(&text, "contentDesc")
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect::<String>();
                if feeds.insert(id, (ownership.effective, preview)).is_some() {
                    return Err(SourceError::AmbiguousIdentity);
                }
            }
        }
        for row in &mut rows {
            if let Some((author, preview)) = feeds.get(&legacy_record_id(&row.moment)?) {
                row.original_author = author.clone();
                row.original_preview = Some(preview.clone());
            }
        }
        Ok(rows)
    }
}

/// Export-only projection of interactions; missing capability remains an explicit error.
pub fn export_comments(
    connection: &Connection,
    zone: legacy::TimeZone,
) -> anyhow::Result<BTreeMap<i64, Vec<legacy::Comment>>> {
    columns(
        connection,
        "SnsMessage_tmp3",
        &[
            "feed_id",
            "create_time",
            "type",
            "from_username",
            "from_nickname",
            "to_username",
            "to_nickname",
            "content",
            "del_status",
        ],
    )?;
    let mut statement = connection.prepare("SELECT feed_id, create_time, type, from_username, from_nickname, to_username, to_nickname, content FROM SnsMessage_tmp3 WHERE COALESCE(del_status,0)=0 ORDER BY create_time")?;
    let mut rows = statement.query([])?;
    let mut result = BTreeMap::<i64, Vec<legacy::Comment>>::new();
    while let Some(row) = rows.next()? {
        let time: Option<i64> = row.get(1)?;
        let kind: Option<i64> = row.get(2)?;
        let type_name = match kind {
            Some(1) => "点赞".into(),
            Some(2) => "评论".into(),
            Some(n) => format!("未知({n})"),
            None => "未知(None)".into(),
        };
        result
            .entry(row.get(0)?)
            .or_default()
            .push(legacy::Comment {
                create_time: time,
                create_time_str: zone.display(time.unwrap_or(0))?,
                kind,
                type_name,
                from_username: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                from_nickname: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                to_username: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                to_nickname: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                content: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
            });
    }
    Ok(result)
}

pub fn open(path: &Path) -> Result<Connection, SourceError> {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| SourceError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_limit_reports_remaining_evidence_without_decoding_it() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); INSERT INTO SnsTimeLine VALUES(2,'a','<TimelineObject/>'),(1,'b','broken')").unwrap();
        let q = business::Query {
            authors: Default::default(),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange::default(),
            keyword: None,
            limit: 10,
            scan_limit: 1,
        };
        let mut source = Timeline::new(&connection, ReadPolicy::QueryCompatibility);
        let page = business::query(&mut source, &q).unwrap();
        assert_eq!(page.scanned, 1);
        assert!(page.scan_truncated);
        assert_eq!(page.moments.len(), 1);
        assert!(page.unreadable.is_empty());
    }

    #[test]
    fn export_comments_preserve_deletion_order_and_unknown_type() {
        let connection = Connection::open_in_memory().unwrap();
        let zone = legacy::TimeZone::default();
        assert!(export_comments(&connection, zone).is_err());
        connection.execute_batch("CREATE TABLE SnsMessage_tmp3(feed_id INTEGER,create_time INTEGER,type INTEGER,from_username TEXT,from_nickname TEXT,to_username TEXT,to_nickname TEXT,content TEXT,del_status INTEGER); INSERT INTO SnsMessage_tmp3 VALUES(1,20,99,'a',NULL,NULL,NULL,'unknown',0),(1,10,2,'b',NULL,NULL,NULL,'first',NULL),(1,5,2,'c',NULL,NULL,NULL,'deleted',1)").unwrap();
        let rows = export_comments(&connection, zone).unwrap();
        let comments = &rows[&1];
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "first");
        assert_eq!(comments[1].kind, Some(99));
        assert_eq!(comments[1].content, "unknown");
    }

    #[test]
    fn export_does_not_silently_ignore_query_keywords() {
        let connection = Connection::open_in_memory().unwrap();
        let mut source = Timeline::new(
            &connection,
            ReadPolicy::ExportCompatibility(legacy::TimeZone::default()),
        );
        let q = business::Query {
            authors: Default::default(),
            author_policy: AuthorPolicy::RecordedCompatibility,
            time: TimeRange::default(),
            keyword: Some("needle".into()),
            limit: 10,
            scan_limit: 10,
        };
        assert!(matches!(
            business::query(&mut source, &q),
            Err(SourceError::InvalidQuery)
        ));
    }

    #[test]
    fn authors_keep_fallback_and_conflict_explicit() {
        assert_eq!(
            author("", "embedded").origin,
            AuthorOrigin::EmbeddedFallback
        );
        let conflict = author("recorded", "embedded");
        assert_eq!(conflict.origin, AuthorOrigin::Conflict);
        assert_eq!(conflict.effective.as_deref(), Some("recorded"));
    }
    #[test]
    fn sqlite_fallback_conflict_recovery_and_schema_are_shared() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT)")
            .unwrap();
        for (id, recorded, xml) in [(1,"","<TimelineObject><username>alice</username><createTime>10</createTime></TimelineObject>"), (2,"bob","<TimelineObject><username>alice</username><createTime>20</createTime></TimelineObject>"), (3,"alice","<TimelineObject><createTime>15</createTime><contentDesc>kept</contentDesc><broken")] {
            connection.execute("INSERT INTO SnsTimeLine VALUES(?1,?2,?3)", rusqlite::params![id,recorded,xml]).unwrap();
        }
        let q = business::Query {
            authors: Default::default(),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange {
                since: Some(10),
                until: Some(20),
            },
            keyword: None,
            limit: 10,
            scan_limit: 10,
        };
        let mut source = Timeline::new(&connection, ReadPolicy::QueryCompatibility);
        let page = business::query(&mut source, &q).unwrap();
        assert_eq!(page.moments.len(), 3);
        assert_eq!(page.author_conflicts.len(), 1);
        assert_eq!(page.moments[1].quality, ContentQuality::RecoveredText);
        connection.execute_batch("DROP TABLE SnsTimeLine").unwrap();
        assert!(matches!(
            source.scan(&q),
            Err(SourceError::UnsupportedFormat)
        ));
    }
    #[test]
    fn notifications_preserve_inclusive_time_unread_and_missing_original() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); CREATE TABLE SnsMessage_tmp3(local_id INTEGER,create_time INTEGER,feed_id INTEGER,from_username TEXT,from_nickname TEXT,content TEXT,is_unread INTEGER); INSERT INTO SnsMessage_tmp3 VALUES(1,10,99,'a','','',1),(2,20,99,'a','','hello',1),(3,20,99,'a','','read',0)").unwrap();
        let rows = business::notifications(
            &mut Notifications(&connection),
            &InteractionQuery {
                time: TimeRange {
                    since: Some(10),
                    until: Some(20),
                },
                include_read: false,
                limit: 10,
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, InteractionKind::Comment);
        assert!(rows[0].original_preview.is_none());
    }

    #[test]
    fn query_and_export_make_author_and_invalid_content_policies_explicit() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT)")
            .unwrap();
        for (id, recorded, text) in [
            (1,"","<root><TimelineObject><username>alice</username><createTime>10</createTime></TimelineObject></root>"),
            (2,"bob","<root><TimelineObject><username>alice</username><createTime>20</createTime></TimelineObject></root>"),
            (3,"alice","<TimelineObject><createTime>15</createTime><contentDesc>recoverable</contentDesc><broken"),
        ] {
            connection.execute("INSERT INTO SnsTimeLine VALUES(?1,?2,?3)",rusqlite::params![id,recorded,text]).unwrap();
        }
        let mut q = business::Query {
            authors: std::collections::BTreeSet::from(["alice".into()]),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange::default(),
            keyword: None,
            limit: 10,
            scan_limit: 10,
        };
        let mut live = Timeline::new(&connection, ReadPolicy::QueryCompatibility);
        let page = business::query(&mut live, &q).unwrap();
        assert_eq!(page.moments.len(), 2);
        q.author_policy = AuthorPolicy::RecordedCompatibility;
        let mut export = Timeline::new(
            &connection,
            ReadPolicy::ExportCompatibility(legacy::TimeZone::default()),
        );
        let page = business::query(&mut export, &q).unwrap();
        assert!(page.moments.is_empty());
        assert_eq!(page.filtered, 2);
        assert_eq!(page.unreadable, [reference(3)]);
        assert_eq!(export.row_number(&reference(3)).unwrap(), 3);
    }

    #[test]
    fn literal_search_is_not_a_wildcard_and_media_identity_stays_exact() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT)")
            .unwrap();
        let xml="<TimelineObject><username>a</username><contentDesc>100%_done</contentDesc><ContentObject><mediaList><media><id>exact-media</id><url key=\"synthetic-token\">https://invalid.example/media</url></media></mediaList></ContentObject></TimelineObject>";
        connection
            .execute("INSERT INTO SnsTimeLine VALUES(1,'a',?1)", [xml])
            .unwrap();
        connection
            .execute(
                "INSERT INTO SnsTimeLine VALUES(2,'a',?1)",
                [xml.replace("100%_done", "100XXdone")],
            )
            .unwrap();
        let q = business::Query {
            authors: Default::default(),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange::default(),
            keyword: Some("%_".into()),
            limit: 10,
            scan_limit: 10,
        };
        let mut source = Timeline::new(&connection, ReadPolicy::QueryCompatibility);
        let page = business::query(&mut source, &q).unwrap();
        assert_eq!(page.moments.len(), 1);
        assert_eq!(
            page.moments[0].media[0].identity.as_deref(),
            Some("exact-media")
        );
        assert!(!format!("{:?}", page.moments[0]).contains("synthetic-token"));
        assert_eq!(
            source.query_projection(&page.moments[0]).unwrap().media[0]["url_key"],
            "synthetic-token"
        );
    }

    #[test]
    fn recorded_selection_precedes_unrelated_invalid_identity_and_content() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE SnsTimeLine(tid,user_name TEXT,content); INSERT INTO SnsTimeLine VALUES('invalid identity','other',X'ff')").unwrap();
        let q = business::Query {
            authors: std::collections::BTreeSet::from(["selected".into()]),
            author_policy: AuthorPolicy::RecordedCompatibility,
            time: TimeRange::default(),
            keyword: None,
            limit: 10,
            scan_limit: 10,
        };
        let mut source = Timeline::new(
            &connection,
            ReadPolicy::ExportCompatibility(legacy::TimeZone::default()),
        );
        let page = business::query(&mut source, &q).unwrap();
        assert_eq!(page.filtered, 1);
        assert!(page.unreadable.is_empty());
        assert!(page.moments.is_empty());
    }

    #[test]
    fn duplicate_source_identity_is_not_last_row_wins() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); INSERT INTO SnsTimeLine VALUES(1,'a','<TimelineObject/>'),(1,'b','<TimelineObject/>')").unwrap();
        let q = business::Query {
            authors: Default::default(),
            author_policy: AuthorPolicy::Effective,
            time: TimeRange::default(),
            keyword: None,
            limit: 10,
            scan_limit: 10,
        };
        let mut source = Timeline::new(&connection, ReadPolicy::QueryCompatibility);
        assert!(matches!(
            business::query(&mut source, &q),
            Err(SourceError::AmbiguousIdentity)
        ));
    }
}
