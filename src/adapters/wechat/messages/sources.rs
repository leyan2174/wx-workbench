//! Host-resolved source requests; only this adapter knows the WeChat layout.
#[derive(Clone, Copy)]
pub struct SourceRequest(&'static str);
impl SourceRequest {
    pub const fn cache_key(self) -> &'static str {
        self.0
    }
}
pub const fn sessions() -> SourceRequest {
    SourceRequest(super::probe::source_key())
}
pub const fn contacts() -> SourceRequest {
    SourceRequest("contact/contact.db")
}

pub fn check_official_inventory(root: &std::path::Path, known: &[String]) -> anyhow::Result<()> {
    use crate::business::messages::{Error, SourceKind};
    use std::collections::HashSet;
    let expected = known
        .iter()
        .map(|key| super::read::logical_name(key, SourceKind::OfficialPush))
        .collect::<anyhow::Result<HashSet<_>>>()?;
    for entry in std::fs::read_dir(root.join("message"))? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| Error::InvalidData)?;
        let logical = format!("message/{name}");
        if let Ok(normalized) = super::read::logical_name(&logical, SourceKind::OfficialPush) {
            anyhow::ensure!(
                expected.contains(&normalized),
                "unknown official message shards; complete inventory required"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_inventory_matches_known_case_without_accepting_new_sources() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("message");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("BIZ_MESSAGE_0.DB"), []).unwrap();
        check_official_inventory(root.path(), &["message\\biz_message_0.db".into()]).unwrap();
        std::fs::write(directory.join("biz_message_1.db"), []).unwrap();
        assert!(
            check_official_inventory(root.path(), &["message/biz_message_0.db".into()]).is_err()
        );
    }
}
