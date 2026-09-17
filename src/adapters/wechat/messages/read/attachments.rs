//! Attachment-list row conversion, not a general tolerant message reader.
use super::*;

#[derive(Clone, Copy)]
pub enum AttachmentReadPolicy {
    LegacyList,
    StrictMetadata,
}
pub struct AttachmentRecord {
    pub reference: MessageRef,
    pub local_id: i64,
    pub local_type: i64,
    pub timestamp: i64,
    pub sender_id: i64,
    pub mapped_sender: Option<String>,
    pub sender_content: String,
}
pub struct AttachmentPage {
    pub rows: Vec<AttachmentRecord>,
    pub skipped_rows: usize,
    pub degraded_content: usize,
}

pub(super) fn sender_text(raw: ValueRef<'_>, compression: i64) -> Result<(String, bool)> {
    let bytes = match raw {
        ValueRef::Blob(bytes) => bytes,
        ValueRef::Text(bytes) if std::str::from_utf8(bytes).is_ok() => bytes,
        ValueRef::Null => return Ok((String::new(), false)),
        _ => return Ok((String::new(), true)),
    };
    ensure!(bytes.len() <= MAX_STORED_BYTES, domain::Error::Limit);
    if compression == 4 && !bytes.is_empty() {
        if let Ok(decoder) = zstd::stream::read::Decoder::new(bytes) {
            let mut decoded = Vec::new();
            let result = decoder
                .take(MAX_DECODED_BYTES as u64 + 1)
                .read_to_end(&mut decoded);
            ensure!(decoded.len() <= MAX_DECODED_BYTES, domain::Error::Limit);
            if result.is_ok() {
                let degraded = std::str::from_utf8(&decoded).is_err();
                return Ok((String::from_utf8_lossy(&decoded).into_owned(), degraded));
            }
        }
        return Ok((String::from_utf8_lossy(bytes).into_owned(), true));
    }
    Ok((
        String::from_utf8_lossy(bytes).into_owned(),
        std::str::from_utf8(bytes).is_err(),
    ))
}

impl Snapshot {
    pub fn read_attachment_page(
        &self,
        stream: usize,
        filter: &domain::Filter,
        legacy: &LegacyReadPolicy,
        limit: usize,
        policy: AttachmentReadPolicy,
    ) -> Result<AttachmentPage> {
        filter.validate()?;
        ensure!(
            filter.kinds.is_empty() && !legacy.local_types.is_empty(),
            domain::Error::Unsupported
        );
        ensure!(legacy.local_types.len() <= 100, domain::Error::Limit);
        let requested = i64::try_from(limit).map_err(|_| domain::Error::Limit)?;
        self.require_content(stream)?;
        let entry = self.streams.get(stream).context("unknown message stream")?;
        ensure!(
            entry.supports_identity() && entry.supports_senders(),
            domain::Error::Unsupported
        );
        let source = &self.sources[entry.source];
        let placeholders = vec!["?"; legacy.local_types.len()].join(",");
        let sql = format!("SELECT {},local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content FROM [{}] WHERE (local_type & 4294967295) IN ({placeholders}) AND (? IS NULL OR create_time>=?) AND (? IS NULL OR create_time<=?) ORDER BY create_time DESC,{} ASC LIMIT ?", entry.row_key, entry.table, entry.row_key);
        let mut values = legacy
            .local_types
            .iter()
            .map(|v| rusqlite::types::Value::Integer(*v))
            .collect::<Vec<_>>();
        for value in [filter.since, filter.since, filter.until, filter.until] {
            values.push(
                value
                    .map(Into::into)
                    .unwrap_or(rusqlite::types::Value::Null),
            );
        }
        values.push(requested.min(100_001).into());
        let mut statement = source.conn.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        let mut page = AttachmentPage {
            rows: Vec::new(),
            skipped_rows: 0,
            degraded_content: 0,
        };
        let mut scanned = 0usize;
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            ensure!(scanned < 100_000, domain::Error::Limit);
            scanned += 1;
            let record = row.get::<_, i64>(0)?;
            let converted = (|| -> rusqlite::Result<_> {
                Ok((
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })();
            let (local_id, local_type, timestamp, sender_id) = match converted {
                Ok(value) => value,
                Err(_) if matches!(policy, AttachmentReadPolicy::LegacyList) => {
                    page.skipped_rows += 1;
                    continue;
                }
                Err(error) => {
                    return Err(anyhow::Error::new(error).context(domain::Error::InvalidData))
                }
            };
            let raw = row.get_ref(5)?;
            let stored = match raw {
                ValueRef::Text(b) | ValueRef::Blob(b) => b.len(),
                _ => 0,
            };
            bytes = bytes.checked_add(stored).ok_or(domain::Error::Limit)?;
            ensure!(bytes <= 64 * 1_048_576, domain::Error::Limit);
            let (sender_content, degraded) = sender_text(raw, row.get::<_, i64>(6).unwrap_or(0))?;
            page.degraded_content += usize::from(degraded);
            page.rows.push(AttachmentRecord {
                reference: self.reference(stream, record),
                local_id,
                local_type,
                timestamp,
                sender_id,
                mapped_sender: source.senders.get(&sender_id).cloned(),
                sender_content,
            });
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_skips_only_row_conversion_and_marks_lossy_sender_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rows.db");
        let table = format!("Msg_{:x}", md5::compute("peer"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content); INSERT INTO [{table}] VALUES('bad',3,100,1,'',0),(7,3,100,1,X'FF',4)")).unwrap();
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
        let legacy = LegacyReadPolicy {
            local_types: vec![3],
        };
        let page = snapshot
            .read_attachment_page(
                0,
                &domain::Filter::default(),
                &legacy,
                10,
                AttachmentReadPolicy::LegacyList,
            )
            .unwrap();
        assert_eq!(
            (page.rows.len(), page.skipped_rows, page.degraded_content),
            (1, 1, 1)
        );
        assert_eq!(page.rows[0].local_id, 7);
        snapshot.revalidate(&page.rows[0].reference).unwrap();
        let error = snapshot
            .read_attachment_page(
                0,
                &domain::Filter::default(),
                &legacy,
                10,
                AttachmentReadPolicy::StrictMetadata,
            )
            .err()
            .expect("strict row conversion must fail");
        assert_eq!(
            error.downcast_ref::<domain::Error>(),
            Some(&domain::Error::InvalidData)
        );
    }
    #[test]
    fn legacy_policy_does_not_hide_empty_table_schema_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rows.db");
        let table = format!("Msg_{:x}", md5::compute("peer"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE [{table}](local_id,local_type,create_time,real_sender_id)"
        ))
        .unwrap();
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
        let error = snapshot
            .read_attachment_page(
                0,
                &domain::Filter::default(),
                &LegacyReadPolicy {
                    local_types: vec![3],
                },
                10,
                AttachmentReadPolicy::LegacyList,
            )
            .err()
            .expect("body schema required even for empty table");
        assert_eq!(
            error.downcast_ref::<domain::Error>(),
            Some(&domain::Error::Unsupported)
        );
    }
}
