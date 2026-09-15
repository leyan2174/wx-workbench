//! Strict account inventory and resource proof. Snapshot acquisition remains with the host.
use super::{resource, strict_message::Message, ImageSource};
use crate::{
    adapters::wechat::messages::{inventory, RawMessage, Snapshot},
    attachment::{
        decoder::V2KeyMaterial,
        native_image::{self, HostOutputGuard, ImageOutput, ImageRequest},
    },
    business::media::{Kind, Source},
};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_INVENTORY_ENTRIES: usize = 20_000;

pub(crate) struct AccountSources {
    database: PathBuf,
    message_keys: Vec<String>,
    resource_key: String,
    inventory: Inventory,
}

impl AccountSources {
    pub(crate) fn capture(
        database: &Path,
        messages: &[String],
        raw_keys: &[String],
    ) -> Result<Self> {
        let keys: Vec<_> = raw_keys
            .iter()
            .filter(|key| normalize(key) == resource::source_key())
            .collect();
        ensure!(keys.len() == 1, "resource key must be present and unique");
        let resource_key = keys[0].clone();
        let inventory = Inventory::capture(database, messages, &resource_key)?;
        Ok(Self {
            database: database.to_path_buf(),
            message_keys: messages.to_vec(),
            resource_key,
            inventory,
        })
    }

    /// Opaque cache lookup descriptor; the host must not interpret its spelling.
    pub(crate) fn resource_key(&self) -> &str {
        &self.resource_key
    }

    pub(crate) fn verify(&self) -> Result<()> {
        self.inventory
            .verify(&self.database, &self.message_keys, &self.resource_key)
    }
}

pub(crate) struct Proof {
    message: Message,
    rowid: i64,
    digest: String,
}

impl Proof {
    pub(crate) fn prepare(
        message: Message,
        messages: &Snapshot,
        raw: &RawMessage,
        resource: &Path,
    ) -> Result<Self> {
        message.verify(messages, raw)?;
        ensure!(message.is_image(), "expected image base_type=3");
        let mut source = ImageSource::from_reference(messages, &raw.reference, resource)?;
        let discovered = source.discover(&raw.reference, Kind::Image)?;
        let item = discovered.first().context("image reference unavailable")?;
        let (rowid, digest) = source.resource_evidence(&item.reference)?;
        Ok(Self {
            message,
            rowid,
            digest: digest.to_owned(),
        })
    }

    /// Reuse the strict exporter, including its resource recheck, read pins and atomic publication.
    pub(crate) fn export(
        &self,
        sources: &AccountSources,
        resource: &Path,
        guard: &HostOutputGuard,
        key: V2KeyMaterial<'_>,
    ) -> Result<ImageOutput> {
        sources.verify()?;
        // Share the account root, not the legacy month-priority candidate policy.
        let attach = super::legacy_dat::attach_root_for(&super::strict_message::account_root(
            &sources.database,
        )?);
        native_image::export_image_with_proof(
            ImageRequest {
                message: self.message.identity(),
                resource_db: resource,
                attach_root: &attach,
                output_root: guard.output_root(),
                key,
            },
            guard,
            (self.rowid, &self.digest),
            || sources.verify(),
        )
    }
}

fn normalize(key: &str) -> String {
    key.replace('\\', "/").to_ascii_lowercase()
}

struct SourceState {
    path: PathBuf,
    identity: same_file::Handle,
    size: u64,
    modified: SystemTime,
}

impl SourceState {
    fn read(path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "inventory source must be a regular file"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "inventory reparse point rejected"
            );
        }
        Ok(Self {
            identity: same_file::Handle::from_path(&path)?,
            path,
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }

    fn unchanged(&self, other: &Self) -> bool {
        self.path == other.path
            && self.identity == other.identity
            && self.size == other.size
            && self.modified == other.modified
    }
}

struct Inventory {
    files: Vec<SourceState>,
}

impl Inventory {
    fn capture(db_dir: &Path, message_keys: &[String], resource_key: &str) -> Result<Self> {
        ensure!(
            inventory::unknown_ordinary_sources(db_dir, message_keys)?.is_empty(),
            "unknown message shards; complete inventory required"
        );
        let mut resource_paths = Vec::new();
        for (index, entry) in fs::read_dir(db_dir.join("message"))?.enumerate() {
            ensure!(
                index < MAX_INVENTORY_ENTRIES,
                "resource inventory entry limit exceeded"
            );
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .context("invalid resource inventory filename")?
                .to_ascii_lowercase();
            if name.starts_with("message_") && name.contains("resource") && name.ends_with(".db") {
                ensure!(
                    name == "message_resource.db",
                    "unknown resource database; complete inventory required"
                );
                resource_paths.push(entry.path());
            }
        }
        ensure!(
            resource_paths.len() == 1,
            "resource source must be present and unique"
        );
        let resource = db_dir.join(resource_key.replace('\\', "/"));
        ensure!(
            same_file::is_same_file(&resource, &resource_paths[0])?,
            "resource key does not identify account source"
        );
        let mut paths: Vec<_> = message_keys
            .iter()
            .map(|key| db_dir.join(key.replace('\\', "/")))
            .collect();
        paths.push(resource);
        paths.sort();
        paths.dedup();
        let mut files = Vec::new();
        for path in paths {
            files.push(SourceState::read(path.clone())?);
            let mut wal = path.into_os_string();
            wal.push("-wal");
            let wal = PathBuf::from(wal);
            match fs::symlink_metadata(&wal) {
                Ok(_) => files.push(SourceState::read(wal)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Self { files })
    }

    fn verify(&self, db_dir: &Path, message_keys: &[String], resource_key: &str) -> Result<()> {
        let current = Self::capture(db_dir, message_keys, resource_key)?;
        ensure!(
            self.files.len() == current.files.len()
                && self
                    .files
                    .iter()
                    .zip(&current.files)
                    .all(|(a, b)| a.unchanged(b)),
            "account source inventory changed before image publication"
        );
        Ok(())
    }
}
