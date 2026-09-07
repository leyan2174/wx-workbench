//! Exact resource metadata and encrypted DAT length; never reads DAT contents.

use super::local_files::{safe_name, Pin, Scan};
use super::native_image::{
    no_sidecars, scan_candidates, MessageIdentity, ResourceLookup, ResourceReader,
    MAX_RESOURCE_BYTES,
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct ImageMetadata {
    pub md5: Option<String>,
    pub size: Option<u64>,
    pub resource_status: &'static str,
    pub size_status: &'static str,
    pub size_kind: &'static str,
    pub binding: &'static str,
}

impl ImageMetadata {
    fn empty(resource_status: &'static str) -> Self {
        Self {
            md5: None,
            size: None,
            resource_status,
            size_status: "not_requested",
            size_kind: "encrypted_dat_metadata",
            binding: "exact_resource_standard_filename_metadata",
        }
    }
}

/// The caller supplies an account-bound resource snapshot and one already-paged chat.
/// The boolean marks any repeated message identity, including within one shard.
pub fn read_page(
    resource_db: Option<&Path>,
    attach_root: Option<&Path>,
    messages: &[(MessageIdentity, bool)],
) -> Result<Vec<ImageMetadata>> {
    ensure!(messages.len() <= 1000, "image metadata page limit exceeded");
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let username = &messages[0].0.username;
    for (message, _) in messages {
        ensure!(
            !username.is_empty()
                && username.len() <= 4096
                && !username.chars().any(char::is_control)
                && message.username == *username
                && !message.source.is_empty()
                && message.source.len() <= 4096
                && !message.source.chars().any(char::is_control)
                && message.local_id > 0
                && message.create_time >= 0
                && message.local_type > 0
                && message.local_type & 0xffff_ffff == 3,
            "invalid image metadata identity"
        );
    }
    let mut output: Vec<_> = messages
        .iter()
        .map(|(_, ambiguous)| {
            ImageMetadata::empty(if *ambiguous {
                "message_ambiguous"
            } else {
                "unavailable"
            })
        })
        .collect();
    let Some(path) = resource_db else {
        return Ok(output);
    };
    if messages.iter().all(|(_, ambiguous)| *ambiguous) {
        return Ok(output);
    }
    let mut scan = Scan::new();
    scan.root(path.parent().context("resource parent missing")?)?;
    safe_name(
        path.file_name()
            .and_then(|n| n.to_str())
            .context("resource filename missing")?,
    )?;
    let db = Pin::open(path, false)?;
    ensure!(
        db.stamp.1 <= MAX_RESOURCE_BYTES,
        "resource database limit exceeded"
    );
    let reader = ResourceReader::open(path)?;
    for ((message, ambiguous), row) in messages.iter().zip(&mut output) {
        if *ambiguous {
            continue;
        }
        match reader.lookup(message)? {
            ResourceLookup::Found(_, md5) => {
                row.md5 = Some(md5);
                row.resource_status = "found";
            }
            ResourceLookup::Missing => row.resource_status = "missing",
            ResourceLookup::Ambiguous => row.resource_status = "ambiguous",
            ResourceLookup::Md5Missing => row.resource_status = "md5_missing",
        }
    }
    let hashes: HashSet<_> = output.iter().filter_map(|row| row.md5.clone()).collect();
    let mut files = Vec::new();
    if let Some(root) = attach_root.filter(|_| !hashes.is_empty()) {
        // Pin every existing ancestor and record each missing component without creating it.
        let mut existing = root;
        let mut missing = Vec::new();
        while matches!(std::fs::symlink_metadata(existing), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            missing.push(existing);
            existing = existing.parent().context("attachment parent missing")?;
        }
        scan.root(existing)?;
        for path in missing.into_iter().rev() {
            scan.optional_directory(path)?;
        }
        let chat = root.join(format!("{:x}", md5::compute(username.as_bytes())));
        let candidates = if scan.optional_directory(&chat)? {
            scan_candidates(&mut scan, &chat, &hashes)?
        } else {
            Default::default()
        };
        for row in &mut output {
            let Some(hash) = &row.md5 else {
                continue;
            };
            let found = candidates.get(hash).map(Vec::as_slice).unwrap_or_default();
            row.size_status = match found {
                [] => "missing",
                [candidate] => {
                    let file = Pin::open(&candidate.path, false)?;
                    row.size = Some(file.stamp.1);
                    files.push(file);
                    "available"
                }
                _ => "ambiguous",
            };
        }
    }
    for file in files {
        file.verify()?;
    }
    db.verify()?;
    no_sidecars(path)?;
    scan.verify()?;
    Ok(output)
}

#[cfg(test)]
#[path = "../../tests/fixtures/mcp-image-listing-parity/metadata_tests.rs"]
mod tests;
