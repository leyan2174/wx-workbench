//! Explicit VoiceInfo export. No message join, decoding or publication.
use super::voice_catalog::MediaShard;
use crate::business::voice_export::{Entry, Source};
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;

/// Physical source evidence for diagnostic export, not a business identity.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportEvidence {
    pub media_source: String,
    pub media_rowid: i64,
    pub media_chat_name_id: i64,
    pub media_local_id: Option<i64>,
    pub media_create_time: Option<i64>,
    pub timestamp_source: &'static str,
    pub svr_id: Option<i64>,
    pub data_index: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_join: Option<super::voice::VoiceEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association_failure: Option<String>,
}

pub struct VoiceRow {
    pub media_rowid: i64,
    pub chat_name_id: i64,
    pub chat_username: String,
    pub create_time: Option<i64>,
    pub local_id: Option<i64>,
    pub svr_id: Option<i64>,
    pub data_index: Option<String>,
    pub voice_data: Vec<u8>,
    pub media_db: String,
}

/// Select database paths, not audio secrets; preserve non-numeric media suffixes.
pub fn media_database_paths<'a>(keys: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut keys: Vec<_> = keys
        .into_iter()
        .map(|key| key.replace('\\', "/").to_ascii_lowercase())
        .filter(|key| key.starts_with("message/media_") && key.ends_with(".db"))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

struct Coordinate {
    shard: usize,
    rowid: i64,
    chat_name_id: i64,
}
pub struct Catalog {
    shards: Vec<(String, Connection)>,
    entries: Vec<Entry>,
    coordinates: Vec<Coordinate>,
    pub unmapped_rows: usize,
}
impl Source for Catalog {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

impl Catalog {
    pub fn manifest(
        &self,
        slot: usize,
        account_id: &str,
    ) -> Result<crate::business::voice_export::ManifestItem<ExportEvidence>> {
        let coord = self.coordinates.get(slot).context("invalid voice slot")?;
        let entry = &self.entries[slot];
        let (source, conn) = &self.shards[coord.shard];
        let absent: bool = conn.query_row(
            "SELECT voice_data IS NULL FROM VoiceInfo WHERE rowid=?1",
            [coord.rowid],
            |r| r.get(0),
        )?;
        Ok(crate::business::voice_export::ManifestItem {
            account_id: account_id.into(),
            message_id: None,
            conversation: Some(entry.username.clone()),
            sender: None,
            timestamp: entry.timestamp,
            duration_ms: None,
            encoding: None,
            relative_path: None,
            status: if absent { "missing" } else { "failed" }.into(),
            association: "unproven".into(),
            evidence: ExportEvidence {
                media_source: source.clone(),
                media_rowid: coord.rowid,
                media_chat_name_id: coord.chat_name_id,
                media_local_id: entry.local_id,
                media_create_time: entry.timestamp,
                timestamp_source: "media",
                svr_id: None,
                data_index: None,
                message_join: None,
                association_failure: None,
            },
            failure: Some(
                if absent {
                    "Media Material: NotFound"
                } else {
                    "Media Material: InvalidMaterial"
                }
                .into(),
            ),
        })
    }

    pub fn open(mut shards: Vec<MediaShard>) -> Result<Self> {
        shards.sort_by(|a, b| a.source.cmp(&b.source));
        let mut catalog = Self {
            shards: Vec::new(),
            entries: Vec::new(),
            coordinates: Vec::new(),
            unmapped_rows: 0,
        };
        for shard in shards {
            let conn = Connection::open_with_flags(
                &shard.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .context("open legacy voice shard")?;
            conn.busy_timeout(std::time::Duration::from_secs(2))?;
            conn.execute_batch("BEGIN")?;
            // Both identifiers below are SQLite rowids, never shadowing user columns.
            for table in ["Name2Id", "VoiceInfo"] {
                let (kind, without_rowid): (String, i64) = conn.query_row(
                    "SELECT type, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1 COLLATE NOCASE",
                    [table], |row| Ok((row.get(0)?, row.get(1)?)),
                ).context("missing legacy voice table")?;
                ensure!(
                    kind == "table" && without_rowid == 0,
                    "unsupported legacy voice table"
                );
                let mut columns = conn.prepare("SELECT name FROM pragma_table_xinfo(?1)")?;
                let names = columns
                    .query_map([table], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ensure!(
                    !names.iter().any(|name| ["rowid", "_rowid_", "oid"]
                        .iter()
                        .any(|alias| name.eq_ignore_ascii_case(alias))),
                    "unsupported legacy voice rowid schema"
                );
            }
            let names: HashMap<i64, String> = {
                let mut stmt = conn.prepare("SELECT rowid, user_name FROM Name2Id")?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                rows
            };
            {
                let mut stmt = conn.prepare("SELECT rowid, chat_name_id, create_time, local_id FROM VoiceInfo ORDER BY create_time, local_id, rowid")?;
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let chat_name_id = row.get(1)?;
                    let Some(username) = names.get(&chat_name_id) else {
                        catalog.unmapped_rows += 1;
                        continue;
                    };
                    catalog.entries.push(Entry {
                        slot: catalog.coordinates.len(),
                        username: username.clone(),
                        timestamp: row.get(2)?,
                        local_id: row.get(3)?,
                    });
                    catalog.coordinates.push(Coordinate {
                        shard: catalog.shards.len(),
                        rowid: row.get(0)?,
                        chat_name_id,
                    });
                }
            }
            catalog.shards.push((shard.source, conn));
        }
        Ok(catalog)
    }

    pub fn material(&self, slot: usize) -> Result<VoiceRow> {
        let coord = self
            .coordinates
            .get(slot)
            .context("invalid legacy voice slot")?;
        let entry = &self.entries[slot];
        let (source, conn) = &self.shards[coord.shard];
        let mut columns = conn.prepare("SELECT name FROM pragma_table_xinfo('VoiceInfo')")?;
        let columns = columns
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let optional = |name: &str| {
            if columns.iter().any(|c| c.eq_ignore_ascii_case(name)) {
                name.to_owned()
            } else {
                "NULL".to_owned()
            }
        };
        let (svr_id, data_index, voice_data) = conn.query_row(
            &format!("SELECT {}, {}, CASE WHEN typeof(voice_data) = 'blob' AND length(voice_data) BETWEEN 1 AND 16777216 THEN voice_data END FROM VoiceInfo WHERE rowid = ?1", optional("svr_id"), optional("data_index")),
            [coord.rowid],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0).unwrap_or(None),
                    row.get::<_, Option<String>>(1).unwrap_or(None),
                    row.get(2)?,
                ))
            },
        )?;
        Ok(VoiceRow {
            media_rowid: coord.rowid,
            chat_name_id: coord.chat_name_id,
            chat_username: entry.username.clone(),
            create_time: entry.timestamp,
            local_id: entry.local_id,
            svr_id,
            data_index,
            voice_data,
            media_db: source.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::voice_export::{select, Selection};

    #[test]
    fn nullable_media_coordinates_remain_unknown_and_missing_is_listed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("media_0.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('chat'); CREATE TABLE VoiceInfo(chat_name_id,create_time,local_id,voice_data); INSERT INTO VoiceInfo VALUES(1,NULL,NULL,NULL)").unwrap();
        drop(conn);
        let catalog = Catalog::open(vec![MediaShard {
            source: "message/media_0.db".into(),
            path,
        }])
        .unwrap();
        let selected = select(&catalog, &Selection::default());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].timestamp, None);
        assert_eq!(selected[0].local_id, None);
        assert!(select(
            &catalog,
            &Selection {
                since: Some(0),
                ..Default::default()
            }
        )
        .is_empty());
        let manifest = serde_json::to_value(catalog.manifest(0, "account").unwrap()).unwrap();
        assert_eq!(manifest["status"], "missing");
        assert_eq!(manifest["association"], "unproven");
        for key in [
            "timestamp",
            "sender",
            "message_id",
            "duration_ms",
            "encoding",
            "relative_path",
        ] {
            assert!(manifest[key].is_null(), "{key}");
        }
        assert!(manifest["evidence"]["media_local_id"].is_null());
    }

    fn shard(root: &std::path::Path, name: &str, times: &[i64]) -> MediaShard {
        let path = root.join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('chat');
            CREATE TABLE VoiceInfo(chat_name_id INTEGER,create_time INTEGER,local_id INTEGER,svr_id INTEGER,data_index TEXT,voice_data BLOB)").unwrap();
        for time in times {
            conn.execute(
                "INSERT INTO VoiceInfo VALUES (1,?1,7,NULL,NULL,?2)",
                rusqlite::params![time, b"\x02#!SILK_V3synthetic".as_slice()],
            )
            .unwrap();
        }
        MediaShard {
            source: format!("message/{name}"),
            path,
        }
    }

    #[test]
    fn cross_shard_global_page_and_raw_audio_without_message_database() {
        let root = tempfile::tempdir().unwrap();
        let a = shard(root.path(), "media_1.db", &[10, 30]);
        let b = shard(root.path(), "media_2.db", &[20, 40]);
        let source = Catalog::open(vec![b, a]).unwrap();
        let page = select(
            &source,
            &Selection {
                since: Some(10),
                until: Some(40),
                offset: 1,
                limit: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(
            page.iter().map(|entry| entry.timestamp).collect::<Vec<_>>(),
            vec![Some(20), Some(30)]
        );
        let raw = source.material(page[0].slot).unwrap();
        assert_eq!(raw.voice_data, b"\x02#!SILK_V3synthetic");
        assert_eq!(raw.media_db, "message/media_2.db");
        assert_eq!(raw.svr_id, None);
        assert_eq!(raw.data_index, None);
        assert_eq!(raw.chat_name_id, 1);
    }

    #[test]
    fn snapshot_keeps_original_bytes_and_does_not_read_unselected_blobs() {
        let root = tempfile::tempdir().unwrap();
        let shard = shard(root.path(), "media_legacy.db", &[10, 20]);
        let writer = Connection::open(&shard.path).unwrap();
        writer
            .execute(
                "UPDATE VoiceInfo SET voice_data = 'not a blob' WHERE create_time = 20",
                [],
            )
            .unwrap();
        writer
            .execute("INSERT INTO VoiceInfo VALUES (99,30,1,0,'',X'01')", [])
            .unwrap();
        let source = Catalog::open(vec![shard]).unwrap();
        writer
            .execute(
                "UPDATE VoiceInfo SET voice_data = X'00' WHERE create_time = 10",
                [],
            )
            .unwrap();
        assert_eq!(source.unmapped_rows, 1);
        let page = select(
            &source,
            &Selection {
                limit: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(
            source.material(page[0].slot).unwrap().voice_data,
            b"\x02#!SILK_V3synthetic"
        );
        assert!(source.material(1).is_err());
        assert!(source.material(usize::MAX).is_err());
    }

    #[test]
    fn inventory_scope_and_schema_failures_are_explicit() {
        let keys: Vec<String> = [
            "message\\media_old.db",
            "message/media_2.db",
            "message/biz_message_0.db",
            "other/media_1.db",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            media_database_paths(&keys),
            vec!["message/media_2.db", "message/media_old.db"]
        );
        let root = tempfile::tempdir().unwrap();
        let shard = shard(root.path(), "media_1.db", &[]);
        Connection::open(&shard.path)
            .unwrap()
            .execute_batch("DROP TABLE VoiceInfo; CREATE TABLE VoiceInfo(rowid INTEGER)")
            .unwrap();
        assert!(Catalog::open(vec![shard]).is_err());
        let missing = root.path().join("missing.db");
        assert!(Catalog::open(vec![MediaShard {
            source: "message/media_9.db".into(),
            path: missing.clone()
        }])
        .is_err());
        assert!(!missing.exists());
    }
}
