//! WeChat favorite storage, capability checks, and legacy diagnostic projection.
use super::legacy_text::{element_text, strip_cdata, unescape_entities};
use crate::{business::favorites::*, daemon::cache::DbCache};
use anyhow::Context;
use rusqlite::{types::ToSql, Connection};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) async fn source_path(db: &DbCache) -> anyhow::Result<PathBuf> {
    db.get("favorite/favorite.db")
        .await?
        .context("找不到 favorite.db，请确认微信数据目录")
}

pub(crate) struct Source {
    conn: Connection,
    legacy_type: Option<i64>,
    evidence: BTreeMap<EvidenceRef, Evidence>,
}

struct Evidence {
    id: i64,
    kind: i64,
    raw_content: String,
}

pub(crate) struct LegacyFields {
    pub id: i64,
    pub kind: i64,
    pub preview: String,
}

impl Source {
    pub(crate) fn open(path: &Path, legacy_type: Option<i64>) -> Result<Self, Error> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            conn,
            legacy_type,
            evidence: BTreeMap::new(),
        })
    }

    pub(crate) fn legacy_fields(&self, evidence: &EvidenceRef) -> Option<LegacyFields> {
        self.evidence.get(evidence).map(|item| LegacyFields {
            id: item.id,
            kind: item.kind,
            preview: preview(&item.raw_content, 100),
        })
    }
}

impl FavoriteSource for Source {
    fn scan(&mut self, query: &Query) -> Result<Page, Error> {
        let mut schema = self
            .conn
            .prepare("PRAGMA table_info(fav_db_item)")
            .map_err(source_error)?;
        let columns = schema
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(source_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(source_error)?;
        if [
            "local_id",
            "type",
            "update_time",
            "content",
            "fromusr",
            "realchatname",
        ]
        .iter()
        .any(|required| !columns.iter().any(|column| column == required))
        {
            return Err(Error::UnsupportedFormat);
        }
        let limit = i64::try_from(query.limit)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::InvalidQuery)?;
        let mut clauses = Vec::new();
        let mut params: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(kind) = self.legacy_type {
            clauses.push("type = ?");
            params.push(Box::new(kind));
        }
        if let Some(text) = &query.text {
            let escaped = text
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            clauses.push("content LIKE ? ESCAPE '\\'");
            params.push(Box::new(format!("%{escaped}%")));
        }
        let condition = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        params.push(Box::new(limit));
        let sql = format!(
            "SELECT local_id, type, update_time, content, fromusr, realchatname
            FROM fav_db_item {condition}
            ORDER BY CASE WHEN update_time > 9999999999 THEN update_time / 1000
                ELSE update_time END DESC, local_id DESC LIMIT ?"
        );
        let values: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mut statement = self.conn.prepare(&sql).map_err(source_error)?;
        let mut records = statement.query(values.as_slice()).map_err(source_error)?;
        let mut items = Vec::new();
        self.evidence.clear();
        let mut has_more = false;
        while let Some(row) = records.next().map_err(source_error)? {
            if items.len() == query.limit {
                has_more = true;
                break;
            }
            let (id, kind, timestamp, content, author, conversation) = (|| {
                Ok::<_, rusqlite::Error>((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })()
            .map_err(|_| Error::InvalidData)?;
            let evidence = EvidenceRef(format!("favorite:{id}"));
            if self
                .evidence
                .insert(
                    evidence.clone(),
                    Evidence {
                        id,
                        kind,
                        raw_content: content.clone(),
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidData);
            }
            items.push(Favorite {
                id: FavoriteId(format!("favorite:{id}")),
                evidence,
                kind: match kind {
                    1 => FavoriteKind::Text,
                    2 => FavoriteKind::Image,
                    5 => FavoriteKind::Article,
                    19 => FavoriteKind::ContactCard,
                    20 => FavoriteKind::Video,
                    _ => FavoriteKind::Other,
                },
                updated_at: if timestamp > 9_999_999_999 {
                    timestamp / 1000
                } else {
                    timestamp
                },
                article_url: (kind == 5).then(|| extract_url(&content)).flatten(),
                text: (kind == 1).then_some(content),
                author,
                conversation,
            });
        }
        Ok(Page { items, has_more })
    }
}

fn source_error(error: rusqlite::Error) -> Error {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            Error::Busy
        }
        Some(rusqlite::ErrorCode::OperationInterrupted) => Error::Cancelled,
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            Error::InvalidData
        }
        _ => Error::Unavailable,
    }
}

pub(crate) fn extract_url(content: &str) -> Option<String> {
    let raw = element_text(content, "link")?;
    let url = unescape_entities(strip_cdata(&raw));
    (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE fav_db_item (
            local_id INTEGER, type INTEGER, update_time INTEGER,
            content TEXT, fromusr TEXT, realchatname TEXT);
            INSERT INTO fav_db_item VALUES
            (1, 1, 1700000000, 'plain text', 'same_name_a', NULL),
            (2, 5, 1700000001000, '<link><![CDATA[https://example.test/?a=1&amp;b=2]]></link>', 'same_name_b', 'group'),
            (3, 87, 1700000002, '100% literal_under_score', NULL, NULL);").unwrap();
        Source {
            conn,
            legacy_type: None,
            evidence: BTreeMap::new(),
        }
    }

    #[test]
    fn returns_business_items_and_opaque_legacy_evidence() {
        let mut source = source();
        let page = crate::business::favorites::list(
            &mut source,
            &Query {
                limit: 10,
                text: None,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(!page.has_more);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.updated_at)
                .collect::<Vec<_>>(),
            [1700000002, 1700000001, 1700000000]
        );
        let article = page
            .items
            .iter()
            .find(|f| f.kind == FavoriteKind::Article)
            .unwrap();
        assert_eq!(article.updated_at, 1700000001);
        assert_eq!(
            article.article_url.as_deref(),
            Some("https://example.test/?a=1&b=2")
        );
        let legacy = source.legacy_fields(&article.evidence).unwrap();
        assert_eq!((legacy.id, legacy.kind), (2, 5));
        assert!(legacy.preview.starts_with("<link>"));
        assert!(article.text.is_none());
        assert_eq!(article.author.as_deref(), Some("same_name_b"));
        assert_ne!(page.items[0].id, page.items[1].id);
        assert!(page.items.iter().any(|f| f.kind == FavoriteKind::Other));
    }

    #[test]
    fn keeps_literal_search_numeric_compatibility_filter_and_page_coverage() {
        let mut source = source();
        let page = source
            .scan(&Query {
                limit: 1,
                text: None,
            })
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.has_more);
        let page = source
            .scan(&Query {
                limit: 10,
                text: Some("% literal_".into()),
            })
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, FavoriteKind::Other);
        source.legacy_type = Some(87);
        assert_eq!(
            source
                .scan(&Query {
                    limit: 10,
                    text: None
                })
                .unwrap()
                .items
                .len(),
            1
        );
        source.legacy_type = Some(88);
        assert!(source
            .scan(&Query {
                limit: 10,
                text: None
            })
            .unwrap()
            .items
            .is_empty());
    }

    #[test]
    fn duplicate_identity_missing_schema_and_bad_rows_are_not_empty_success() {
        let mut source = source();
        source
            .conn
            .execute(
                "INSERT INTO fav_db_item SELECT * FROM fav_db_item WHERE local_id=1",
                [],
            )
            .unwrap();
        assert!(matches!(
            source.scan(&Query {
                limit: 10,
                text: None
            }),
            Err(Error::InvalidData)
        ));
        source
            .conn
            .execute_batch(
                "DELETE FROM fav_db_item; INSERT INTO fav_db_item VALUES (NULL,1,1,'x',NULL,NULL)",
            )
            .unwrap();
        assert!(matches!(
            source.scan(&Query {
                limit: 10,
                text: None
            }),
            Err(Error::InvalidData)
        ));
        source.conn.execute_batch("DROP TABLE fav_db_item").unwrap();
        assert!(matches!(
            source.scan(&Query {
                limit: 10,
                text: None
            }),
            Err(Error::UnsupportedFormat)
        ));
    }

    #[test]
    fn legacy_fragment_link_policy_is_explicit_not_strict_media_authority() {
        assert_eq!(
            extract_url("prefix<link>https://example.test/</link>suffix").as_deref(),
            Some("https://example.test/")
        );
        assert_eq!(extract_url("<link>file:///private</link>"), None);
        assert_eq!(extract_url("<link>javascript:alert(1)</link>"), None);
    }

    #[test]
    fn lookahead_does_not_parse_an_unrequested_row() {
        let mut source = source();
        source
            .conn
            .execute_batch("INSERT INTO fav_db_item VALUES (NULL,1,1699999999,'bad',NULL,NULL)")
            .unwrap();
        let page = source
            .scan(&Query {
                limit: 3,
                text: None,
            })
            .unwrap();
        assert_eq!(page.items.len(), 3);
        assert!(page.has_more);
        assert!(matches!(
            source.scan(&Query {
                limit: 4,
                text: None
            }),
            Err(Error::InvalidData)
        ));
    }

    #[test]
    fn real_locked_source_is_busy_not_an_unsupported_schema() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.db");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "CREATE TABLE fav_db_item (local_id,type,update_time,content,fromusr,realchatname)",
            )
            .unwrap();
        let mut reader = Source::open(&path, None).unwrap();
        reader.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert!(matches!(
            reader.scan(&Query {
                limit: 10,
                text: None
            }),
            Err(Error::Busy)
        ));
        writer.execute_batch("ROLLBACK").unwrap();
        assert!(reader
            .scan(&Query {
                limit: 10,
                text: None
            })
            .unwrap()
            .items
            .is_empty());
    }
}
