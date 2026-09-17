//! Detached decode diagnostics over an explicit host-selected stream scope.
//! These values carry no stable message identity or strict-content proof.
use super::*;

/// Compatibility projection only; this value is not a portable business identity.
pub fn legacy_unmapped_key(reference: &domain::UnmappedConversation) -> &str {
    &reference.0
}

pub fn validate_table(name: &str) -> Result<()> {
    if !valid_table(name) {
        return Err(anyhow::Error::new(domain::Error::Unsupported).context("消息表名不合法"));
    }
    Ok(())
}

pub struct DecodeEvidence {
    pub logical_source: String,
    pub local_type: i64,
    pub timestamp: i64,
    content: Vec<u8>,
    compression: i64,
    unavailable_content: bool,
}

impl DecodeEvidence {
    pub fn legacy_content(&self) -> Result<(String, bool)> {
        let (text, degraded) =
            attachments::sender_text(ValueRef::Blob(&self.content), self.compression)?;
        Ok((text, degraded || self.unavailable_content))
    }
}

impl Snapshot {
    pub fn decode_diagnostics(
        &self,
        streams: &[usize],
        local_id: i64,
        timestamp: Option<i64>,
    ) -> Result<Vec<DecodeEvidence>> {
        let mut output = Vec::new();
        let mut stored_bytes = 0usize;
        for &stream in streams {
            self.require_content(stream)?;
            let entry = self.streams.get(stream).context("unknown message stream")?;
            ensure!(entry.supports_identity(), domain::Error::Unsupported);
            let source = &self.sources[entry.source];
            let sql = format!("SELECT local_type,create_time,message_content,WCDB_CT_message_content FROM [{}] WHERE local_id=?1 AND (?2 IS NULL OR create_time=?2) ORDER BY {} ASC LIMIT 100001", entry.table, entry.row_key);
            let mut statement = source.conn.prepare(&sql)?;
            let mut rows = statement.query(rusqlite::params![local_id, timestamp])?;
            while let Some(row) = rows.next()? {
                ensure!(output.len() < 100_000, domain::Error::Limit);
                let local_type = row.get::<_, i64>(0).context(domain::Error::InvalidData)?;
                let timestamp = row.get::<_, i64>(1).context(domain::Error::InvalidData)?;
                let raw = row.get_ref(2)?;
                let size = match raw {
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
                    _ => 0,
                };
                stored_bytes = stored_bytes.checked_add(size).ok_or(domain::Error::Limit)?;
                ensure!(stored_bytes <= 64 * 1_048_576, domain::Error::Limit);
                ensure!(size <= MAX_STORED_BYTES, domain::Error::Limit);
                let (content, unavailable_content) = match raw {
                    ValueRef::Blob(bytes) => (bytes.to_vec(), false),
                    ValueRef::Text(bytes) if std::str::from_utf8(bytes).is_ok() => {
                        (bytes.to_vec(), false)
                    }
                    ValueRef::Null => (Vec::new(), false),
                    _ => (Vec::new(), true),
                };
                output.push(DecodeEvidence {
                    logical_source: source.logical_name.clone(),
                    local_type,
                    timestamp,
                    content,
                    compression: row.get::<_, i64>(3).unwrap_or(0),
                    unavailable_content,
                });
            }
        }
        Ok(output)
    }
}
