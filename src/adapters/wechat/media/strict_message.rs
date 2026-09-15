//! Detached private evidence for strict media selection. No daemon or cache authority.
use super::{attachment_content as content, resource::MessageIdentity};
use crate::{
    adapters::wechat::messages::{DetachedContent, RawMessage, Snapshot},
    business::{attachment_content::Selection, messages::Conversation},
};
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};

pub(crate) struct Message {
    identity: MessageIdentity,
    content: DetachedContent,
}

impl Message {
    pub(crate) fn capture(snapshot: &Snapshot, raw: &RawMessage) -> Result<Self> {
        let Conversation::Known(username) = snapshot.conversation(&raw.reference)? else {
            anyhow::bail!(crate::business::messages::Error::InvalidData)
        };
        Ok(Self {
            identity: MessageIdentity {
                username: username.clone(),
                source: raw.logical_source.clone(),
                local_id: raw.local_id.context("message local identity unavailable")?,
                create_time: raw.timestamp,
                local_type: raw.local_type,
            },
            content: raw.detached_content(),
        })
    }

    pub(crate) fn is_image(&self) -> bool {
        self.identity.local_type > 0 && self.identity.local_type & 0xffff_ffff == 3
    }

    pub(crate) fn supports_attachments(&self) -> bool {
        self.identity.local_type > 0 && self.identity.local_type & 0xffff_ffff == 49
    }

    pub(crate) fn attachment(
        &self,
        selection: Selection,
    ) -> Result<content::Result<content::AttachmentMetadata>> {
        ensure!(
            self.supports_attachments(),
            "expected app message base_type=49"
        );
        let limit = match selection {
            Selection::File => 20_000,
            Selection::RecordItem(_) => 500_000,
        };
        let bytes = self.content.bounded_decode(limit)?;
        let body = std::str::from_utf8(&bytes).context("invalid attachment text encoding")?;
        let input = content::MessageInput {
            username: &self.identity.username,
            source: &self.identity.source,
            local_id: self.identity.local_id,
            create_time: self.identity.create_time,
            body,
        };
        Ok(match selection {
            Selection::File => content::parse_file_message(&input),
            Selection::RecordItem(index) => content::parse_record_item(&input, index),
        })
    }

    pub(super) fn identity(&self) -> &MessageIdentity {
        &self.identity
    }

    pub(super) fn verify(&self, snapshot: &Snapshot, raw: &RawMessage) -> Result<()> {
        use crate::business::media::{Error, Failure, Stage};
        ensure!(
            raw.logical_source == self.identity.source
                && raw.local_id == Some(self.identity.local_id)
                && raw.timestamp == self.identity.create_time
                && raw.local_type == self.identity.local_type
                && matches!(snapshot.conversation(&raw.reference)?,
                    Conversation::Known(name) if name == &self.identity.username),
            Error::new(Stage::Revalidation, Failure::StaleEvidence)
        );
        Ok(())
    }
}

/// Account layout is selected only from the host's fixed database root.
pub(crate) fn account_root(database: &Path) -> Result<PathBuf> {
    Ok(database
        .parent()
        .context("missing account root")?
        .to_path_buf())
}
