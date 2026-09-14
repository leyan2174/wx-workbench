//! Explicit raw-export projection. Nullable timestamps are not ordinary Message values.
use super::*;

const MAX_EXPORT_BODY: usize = 64 * 1_048_576;
#[derive(Clone, Copy)]
pub enum ExportProfile {
    Compact,
    Directory,
    Delta,
}
pub struct ExportRecord {
    pub local_id: i64,
    pub local_type: i64,
    pub timestamp: Option<i64>,
    pub mapped_sender: Option<String>,
    pub content: StoredContent,
    pub decoded: Option<String>,
    pub server_id: StoredScalar,
    pub sort_seq: Option<i64>,
    pub status: Option<i64>,
}

impl ExportRecord {
    pub fn append_details(
        &self,
        extras: &mut serde_json::Map<String, serde_json::Value>,
        group_prefix: &str,
        is_group: bool,
        username: &str,
    ) -> Result<()> {
        use serde_json::Value;
        let mapped = self.mapped_sender.as_deref().unwrap_or("");
        let sender = if is_group && (mapped.is_empty() || mapped == username) {
            group_prefix
        } else {
            mapped
        };
        let server_id = match &self.server_id {
            StoredScalar::AbsentColumn | StoredScalar::Null => Value::Null,
            StoredScalar::Integer(value) => Value::from(*value),
            StoredScalar::Text(value) => Value::from(value.clone()),
            _ => anyhow::bail!("server_id 必须为 SQLite INTEGER、TEXT 或 NULL"),
        };
        extras.insert("local_type".into(), self.local_type.into());
        extras.insert("server_id".into(), server_id);
        extras.insert("sort_seq".into(), serde_json::to_value(self.sort_seq)?);
        extras.insert("status".into(), serde_json::to_value(self.status)?);
        extras.insert(
            "sender_username".into(),
            serde_json::to_value((!sender.is_empty()).then_some(sender))?,
        );
        extras.insert("raw_content".into(), serde_json::to_value(&self.decoded)?);
        Ok(())
    }
}

pub fn decode_value(
    raw: ValueRef<'_>,
    compression: Option<i64>,
    profile: ExportProfile,
) -> Result<(StoredContent, Option<String>)> {
    let content = match raw {
        ValueRef::Null => StoredContent::Null,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            ensure!(bytes.len() <= MAX_EXPORT_BODY, domain::Error::Limit);
            if matches!(raw, ValueRef::Text(_)) {
                StoredContent::Text(bytes.to_vec())
            } else {
                StoredContent::Blob(bytes.to_vec())
            }
        }
        _ => anyhow::bail!(domain::Error::InvalidData),
    };
    let decoded = decoded(&content, compression, profile)?;
    Ok((content, decoded))
}

fn decoded(
    content: &StoredContent,
    compression: Option<i64>,
    profile: ExportProfile,
) -> Result<Option<String>> {
    let bytes = match content {
        StoredContent::Null
            if matches!(profile, ExportProfile::Directory | ExportProfile::Delta) =>
        {
            return Ok(None)
        }
        StoredContent::Null => &[][..],
        StoredContent::Text(bytes) if matches!(profile, ExportProfile::Delta) => {
            return Ok(Some(std::str::from_utf8(bytes)?.to_owned()))
        }
        StoredContent::Text(bytes) | StoredContent::Blob(bytes) => bytes.as_slice(),
        _ => anyhow::bail!(domain::Error::Unsupported),
    };
    let decoded = if compression == Some(4) {
        let result = (|| -> Result<Vec<u8>> {
            let decoder = zstd::stream::read::Decoder::new(bytes)?;
            let mut output = Vec::new();
            let status = decoder
                .take(MAX_EXPORT_BODY as u64 + 1)
                .read_to_end(&mut output);
            ensure!(output.len() <= MAX_EXPORT_BODY, domain::Error::Limit);
            status?;
            Ok(output)
        })();
        match result {
            Ok(value) => value,
            Err(error)
                if matches!(profile, ExportProfile::Delta)
                    && error.downcast_ref::<domain::Error>() != Some(&domain::Error::Limit) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        }
    } else {
        bytes.to_vec()
    };
    if matches!(profile, ExportProfile::Delta) {
        Ok(Some(String::from_utf8_lossy(&decoded).into_owned()))
    } else {
        Ok(Some(String::from_utf8(decoded)?))
    }
}

impl Snapshot {
    pub fn visit_export(
        &self,
        stream: usize,
        filter: &domain::Filter,
        profile: ExportProfile,
        mut visit: impl FnMut(ExportRecord) -> Result<()>,
    ) -> Result<()> {
        // Legacy delta treats an inverted inclusive SQL window as an empty result.
        if !matches!(profile, ExportProfile::Delta) {
            filter.validate()?;
        }
        ensure!(filter.kinds.is_empty(), domain::Error::Unsupported);
        self.require_content(stream)?;
        let entry = self.streams.get(stream).context("unknown message stream")?;
        ensure!(
            entry.supports_identity() && entry.supports_senders(),
            domain::Error::Unsupported
        );
        let source = &self.sources[entry.source];
        let details = ["server_id", "sort_seq", "status"]
            .map(|name| {
                if matches!(profile, ExportProfile::Directory) && entry.columns.contains(name) {
                    format!("[{name}]")
                } else {
                    "NULL".into()
                }
            })
            .join(",");
        let tie = if matches!(profile, ExportProfile::Delta) {
            entry.row_key
        } else {
            "local_id"
        };
        let sql = format!("SELECT local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content,{details} FROM [{}] WHERE (?1 IS NULL OR create_time>=?1) AND (?2 IS NULL OR create_time<=?2) ORDER BY create_time ASC,{tie} ASC,{} ASC", entry.table, entry.row_key);
        let mut statement = source.conn.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params![filter.since, filter.until])?;
        while let Some(row) = rows.next()? {
            let (content, decoded) = decode_value(row.get_ref(4)?, row.get(5)?, profile)?;
            let sender: Option<i64> = row.get(3)?;
            visit(ExportRecord {
                local_id: row.get(0)?,
                local_type: row.get(1)?,
                timestamp: row.get(2)?,
                mapped_sender: sender.and_then(|id| source.senders.get(&id).cloned()),
                content,
                decoded,
                server_id: scalar(row.get_ref(6)?)?,
                sort_seq: row.get(7)?,
                status: row.get(8)?,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverted_delta_window_is_empty_without_bypassing_projection_checks() {
        for has_body in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("window.db");
            let table = format!("Msg_{:x}", md5::compute("peer"));
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("CREATE TABLE [{table}](local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content); INSERT INTO [{table}] VALUES(7,1,150,NULL,'text',0)")).unwrap();
            if !has_body {
                conn.execute_batch(&format!(
                    "ALTER TABLE [{table}] DROP COLUMN message_content"
                ))
                .unwrap();
            }
            drop(conn);
            let snapshot = Snapshot::open(
                vec![SourceFile {
                    logical_name: "message/message_0.db".into(),
                    path,
                    kind: SourceKind::Ordinary,
                }],
                ["peer".into()],
            )
            .unwrap();
            let filter = domain::Filter {
                since: Some(200),
                until: Some(100),
                kinds: Vec::new(),
            };
            let mut count = 0;
            let result = snapshot.visit_export(0, &filter, ExportProfile::Delta, |_| {
                count += 1;
                Ok(())
            });
            if has_body {
                result.unwrap();
                assert_eq!(count, 0);
                let error = snapshot
                    .visit_export(0, &filter, ExportProfile::Compact, |_| Ok(()))
                    .unwrap_err();
                assert_eq!(
                    error.downcast_ref::<domain::Error>(),
                    Some(&domain::Error::InvalidData)
                );
            } else {
                assert_eq!(
                    result.unwrap_err().downcast_ref::<domain::Error>(),
                    Some(&domain::Error::Unsupported)
                );
            }
        }
    }

    #[test]
    fn raw_profiles_keep_storage_and_legacy_decode_distinctions() {
        let (raw, text) =
            decode_value(ValueRef::Text(b"literal"), Some(4), ExportProfile::Delta).unwrap();
        assert!(matches!(raw, StoredContent::Text(_)));
        assert_eq!(text.as_deref(), Some("literal"));
        assert!(decode_value(ValueRef::Text(b"literal"), Some(4), ExportProfile::Compact).is_err());
        let (raw, text) = decode_value(
            ValueRef::Blob(b"broken zstd"),
            Some(4),
            ExportProfile::Delta,
        )
        .unwrap();
        assert!(matches!(raw, StoredContent::Blob(_)));
        assert!(text.is_none());
        assert!(decode_value(
            ValueRef::Blob(b"broken zstd"),
            Some(4),
            ExportProfile::Directory
        )
        .is_err());
        assert!(
            decode_value(ValueRef::Null, Some(4), ExportProfile::Directory)
                .unwrap()
                .1
                .is_none()
        );
        assert_eq!(
            decode_value(ValueRef::Null, None, ExportProfile::Compact)
                .unwrap()
                .1
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn raw_export_keeps_null_time_and_text_server_id_without_body_projection_leaks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("raw.db");
        let table = format!("Msg_{:x}", md5::compute("peer"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content,server_id,sort_seq,status); INSERT INTO [{table}] VALUES(7,1,NULL,NULL,NULL,0,'0009223372036854775808',NULL,NULL)")).unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        let snapshot = Snapshot::open(
            vec![SourceFile {
                logical_name: "message/message_0.db".into(),
                path: path.clone(),
                kind: SourceKind::Ordinary,
            }],
            ["peer".into()],
        )
        .unwrap();
        let mut count = 0;
        snapshot
            .visit_export(
                0,
                &domain::Filter::default(),
                ExportProfile::Directory,
                |row| {
                    count += 1;
                    assert_eq!(row.timestamp, None);
                    let mut extras = serde_json::Map::new();
                    row.append_details(&mut extras, "", false, "peer")?;
                    assert_eq!(extras["server_id"], "0009223372036854775808");
                    assert!(extras["raw_content"].is_null());
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(count, 1);
        drop(snapshot);
        assert_eq!(before, std::fs::read(path).unwrap());
    }
}
