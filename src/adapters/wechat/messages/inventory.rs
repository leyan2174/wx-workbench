//! Physical inventory diagnostics. Normal queries must propagate discovery errors.
use crate::business::messages::SourceKind;
use std::{collections::HashSet, path::Path};

/// Preserve configured candidates, including malformed names that strict reads
/// must reject. Filtering only fully validated names would hide missing data.
pub fn configured_sources<'a>(
    keys: impl Iterator<Item = &'a str>,
    kind: SourceKind,
) -> Vec<String> {
    let mut selected: Vec<_> = keys
        .filter(|key| configured_candidate(key, kind))
        .map(str::to_owned)
        .collect();
    selected.sort();
    selected
}

fn configured_candidate(key: &str, kind: SourceKind) -> bool {
    let key = key.replace('\\', "/");
    let prefix = match kind {
        SourceKind::Ordinary => "message/message_",
        SourceKind::OfficialPush => "message/biz_message_",
    };
    key.starts_with(prefix)
        && key.ends_with(".db")
        && !key.contains("_fts")
        && !key.contains("_resource")
}

pub fn unknown_ordinary_sources(root: &Path, known: &[String]) -> anyhow::Result<Vec<String>> {
    let known: HashSet<_> = known
        .iter()
        .map(|key| key.replace('\\', "/").to_ascii_lowercase())
        .collect();
    let mut unknown = Vec::new();
    for entry in std::fs::read_dir(root.join("message"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_message_shard(name) {
            continue;
        }
        anyhow::ensure!(
            entry.file_type()?.is_file(),
            "message shard is not a regular file"
        );
        let relative = format!("message/{}", name.to_ascii_lowercase());
        if !known.contains(&relative) {
            unknown.push(relative);
        }
    }
    unknown.sort();
    Ok(unknown)
}

pub(crate) fn is_message_shard(file_name: &str) -> bool {
    super::read::logical_name(
        &format!("message/{file_name}"),
        crate::business::messages::SourceKind::Ordinary,
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_db_key_filter_ignores_biz_and_auxiliary_files() {
        let is_msg_db_key = |key| configured_candidate(key, SourceKind::Ordinary);
        assert!(is_msg_db_key("message/message_0.db"));
        assert!(is_msg_db_key("message\\message_12.db"));
        assert!(!is_msg_db_key("message/biz_message_0.db"));
        assert!(!is_msg_db_key("message/message_0.db-wal"));
        assert!(!is_msg_db_key("message/message_0_fts.db"));
        assert!(!is_msg_db_key("message/message_0_resource.db"));
    }

    #[test]
    fn biz_message_db_key_filter_matches_only_biz_shards() {
        let is_biz_msg_db_key = |key| configured_candidate(key, SourceKind::OfficialPush);
        assert!(is_biz_msg_db_key("message/biz_message_0.db"));
        assert!(is_biz_msg_db_key("message\\biz_message_3.db"));
        assert!(!is_biz_msg_db_key("message/message_0.db"));
        assert!(!is_biz_msg_db_key("message/biz_message_0.db-wal"));
        assert!(!is_biz_msg_db_key("message/biz_message_0_fts.db"));
        assert!(!is_biz_msg_db_key("message/biz_message_0_resource.db"));
    }

    #[test]
    fn candidate_selection_keeps_original_keys_and_invalid_candidates_for_rejection() {
        let keys = [
            "message/message_a.db",
            "message\\message_2.db",
            "message/message_1.db",
        ];
        assert_eq!(
            configured_sources(keys.into_iter(), SourceKind::Ordinary),
            [
                "message/message_1.db",
                "message/message_a.db",
                "message\\message_2.db"
            ]
        );
        assert!(super::super::read::logical_name(keys[0], SourceKind::Ordinary).is_err());
        // The established configured-key profile is case-sensitive; disk
        // discovery separately canonicalizes names and detects unknown sources.
        assert!(!configured_candidate(
            "MESSAGE/MESSAGE_1.DB",
            SourceKind::Ordinary
        ));
    }
}
