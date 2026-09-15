//! Physical inventory diagnostics. Normal queries must propagate discovery errors.
use std::{collections::HashSet, path::Path};

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
