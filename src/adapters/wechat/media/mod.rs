//! Account-bound media evidence. Paths and resource coordinates stay in this adapter.
pub(crate) mod attachment_content;
pub(crate) mod attachment_kind;
pub(crate) mod directory_layout;
pub(crate) mod image_batch;
pub(crate) mod legacy_dat;
pub(crate) mod local_emoticon;
pub(crate) mod local_read;
pub mod resource;
pub(crate) mod strict_image;
pub(crate) mod strict_message;
pub mod voice;
pub mod voice_catalog;
pub mod voice_export;
use self::resource::{MessageIdentity, ResourceLookup, ResourceReader};
use super::messages::Snapshot;
use crate::{
    attachment::{local_files::Pin, native_image::MAX_RESOURCE_BYTES},
    business::{
        media::{
            self, AssociationPolicy, Discovered, Error, Failure, Kind, Reference, Source, Stage,
        },
        messages::{Completeness, Conversation, MessageRef, MessageSelector, SourceKind},
    },
};
use std::{path::Path, sync::Arc};

fn message_failure(error: anyhow::Error, stage: Stage) -> Error {
    error
        .downcast_ref::<crate::business::messages::Error>()
        .map(|error| media::message_error(*error, stage))
        .unwrap_or_else(|| Error::new(stage, Failure::Unavailable))
}

struct ImageProof {
    reference: Reference,
    rowid: i64,
    digest: String,
}

/// Legacy listing identity includes the complete raw type. This is not the
/// selector used to authorize image decoding, which remains type-agnostic.
pub(crate) fn image_listing_reference(
    snapshot: &Snapshot,
    selector: &MessageSelector<'_>,
    raw_type: i64,
) -> anyhow::Result<MessageRef> {
    use crate::adapters::wechat::messages::{
        read::attachments::AttachmentReadPolicy, LegacyReadPolicy,
    };
    use crate::business::messages::{Error as MessageError, Filter};
    let timestamp = selector.timestamp.ok_or(MessageError::InvalidData)?;
    anyhow::ensure!(
        raw_type > 0 && raw_type & 0xffff_ffff == 3,
        MessageError::InvalidData
    );
    let mut found = None;
    for stream in snapshot.streams_for(selector.username, SourceKind::Ordinary) {
        let page = snapshot.read_attachment_page(
            stream,
            &Filter {
                since: Some(timestamp),
                until: Some(timestamp),
                kinds: Vec::new(),
            },
            &LegacyReadPolicy {
                local_types: vec![3],
            },
            100_001,
            AttachmentReadPolicy::StrictMetadata,
        )?;
        for row in page.rows {
            if row.local_id == selector.local_id && row.local_type == raw_type {
                anyhow::ensure!(found.is_none(), MessageError::Ambiguous);
                found = Some(row.reference);
            }
        }
    }
    let reference = found.ok_or(MessageError::NotFound)?;
    snapshot.revalidate(&reference)?;
    Ok(reference)
}

/// Resolve against the complete caller inventory, never only an exported page.
pub(crate) fn image_digest(
    snapshot: &Snapshot,
    selector: &MessageSelector<'_>,
    expected_source: &str,
    expected_type: i64,
    resources: &[std::path::PathBuf],
) -> Result<String, Error> {
    if selector.timestamp.is_none() {
        return Err(Error::new(Stage::Association, Failure::InvalidReference));
    }
    let reference = snapshot
        .resolve(selector, SourceKind::Ordinary)
        .map_err(|error| message_failure(error, Stage::Association))?;
    let raw = snapshot
        .read_metadata(reference.evidence())
        .map_err(|error| message_failure(error, Stage::Revalidation))?;
    if !raw
        .logical_source
        .replace('\\', "/")
        .eq_ignore_ascii_case(&expected_source.replace('\\', "/"))
        || raw.local_type != expected_type
    {
        return Err(Error::new(Stage::Revalidation, Failure::StaleEvidence));
    }
    let mut digest = None;
    let mut proofs = Vec::new();
    for path in resources {
        let mut source = ImageSource::from_reference(snapshot, &reference, path)?;
        match source.discover(&reference, Kind::Image) {
            Ok(mut items) => {
                if items.len() != 1 || digest.is_some() {
                    return Err(Error::new(Stage::Association, Failure::Ambiguous));
                }
                let item = items.pop().expect("one image reference");
                digest = Some(source.resource_evidence(&item.reference)?.1.to_owned());
                proofs.push((source, item.reference));
            }
            Err(error)
                if error.stage == Stage::Association && error.failure == Failure::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    for (source, reference) in &proofs {
        source.revalidate(reference)?;
    }
    snapshot
        .revalidate(&reference)
        .map_err(|error| message_failure(error, Stage::Revalidation))?;
    digest.ok_or(Error::new(Stage::Association, Failure::NotFound))
}

/// Borrows a live message snapshot; cannot extend its lifetime or rebind a reference.
pub struct ImageSource<'a> {
    messages: &'a Snapshot,
    message: MessageRef,
    identity: MessageIdentity,
    resources: ResourceReader,
    resource_pin: Pin,
    owner: Arc<()>,
    proof: Option<ImageProof>,
}

impl<'a> ImageSource<'a> {
    #[cfg(test)]
    pub fn open(
        messages: &'a Snapshot,
        selector: &MessageSelector<'_>,
        resource: &Path,
    ) -> Result<Self, Error> {
        let message = messages
            .resolve(selector, SourceKind::Ordinary)
            .map_err(|error| message_failure(error, Stage::Discovery))?;
        Self::from_reference(messages, &message, resource)
    }

    pub fn from_reference(
        messages: &'a Snapshot,
        message: &MessageRef,
        resource: &Path,
    ) -> Result<Self, Error> {
        messages
            .revalidate(message)
            .map_err(|error| message_failure(error, Stage::Discovery))?;
        let username = match messages
            .conversation(message)
            .map_err(|error| message_failure(error, Stage::Discovery))?
        {
            Conversation::Known(username) => username.clone(),
            Conversation::Unmapped(_) => {
                return Err(Error::new(Stage::Discovery, Failure::InvalidReference))
            }
        };
        let raw = messages
            .read_evidence(message.evidence())
            .map_err(|error| message_failure(error, Stage::Discovery))?;
        if raw.local_type & 0xffff_ffff != 3 {
            return Err(Error::new(Stage::Association, Failure::InvalidReference));
        }
        let identity = MessageIdentity {
            username,
            source: raw.logical_source,
            local_id: raw
                .local_id
                .ok_or_else(|| Error::new(Stage::Discovery, Failure::InvalidReference))?,
            create_time: raw.timestamp,
            local_type: raw.local_type,
        };
        let resource_pin = Pin::open(resource, false)
            .map_err(|_| Error::new(Stage::Discovery, Failure::UnsafeSource))?;
        let length = std::fs::metadata(resource)
            .map_err(|_| Error::new(Stage::Discovery, Failure::Unavailable))?
            .len();
        if length > MAX_RESOURCE_BYTES {
            return Err(Error::new(Stage::Discovery, Failure::LimitExceeded));
        }
        let resources = ResourceReader::open(resource)
            .map_err(|_| Error::new(Stage::Discovery, Failure::Unavailable))?;
        Ok(Self {
            messages,
            message: message.clone(),
            identity,
            resources,
            resource_pin,
            owner: Arc::new(()),
            proof: None,
        })
    }

    #[cfg(test)]
    pub fn message(&self) -> &MessageRef {
        &self.message
    }

    /// The old wire projection is produced only after the reference is revalidated.
    pub fn resource_evidence(&self, reference: &Reference) -> Result<(i64, &str), Error> {
        self.revalidate(reference)?;
        let proof = self
            .proof
            .as_ref()
            .ok_or_else(|| Error::new(Stage::Revalidation, Failure::InvalidReference))?;
        Ok((proof.rowid, &proof.digest))
    }
}

#[cfg(test)]
mod tests;

impl Source for ImageSource<'_> {
    fn discover(&mut self, message: &MessageRef, kind: Kind) -> Result<Vec<Discovered>, Error> {
        self.messages
            .revalidate(message)
            .map_err(|error| message_failure(error, Stage::Discovery))?;
        if message != &self.message || kind != Kind::Image {
            return Err(Error::new(Stage::Discovery, Failure::InvalidReference));
        }
        self.resource_pin
            .verify()
            .map_err(|_| Error::new(Stage::Revalidation, Failure::StaleEvidence))?;
        let (rowid, digest) = match self
            .resources
            .lookup(&self.identity)
            .map_err(|_| Error::new(Stage::Discovery, Failure::Unavailable))?
        {
            ResourceLookup::Found(rowid, digest) => (rowid, digest),
            ResourceLookup::Missing => {
                return Err(Error::new(Stage::Association, Failure::NotFound))
            }
            ResourceLookup::Ambiguous => {
                return Err(Error::new(Stage::Association, Failure::Ambiguous))
            }
            ResourceLookup::Md5Missing => {
                return Err(Error::new(Stage::Association, Failure::ConflictingEvidence))
            }
        };
        let reference = Reference::new(
            &self.owner,
            message.clone(),
            kind,
            None,
            AssociationPolicy::StrictMessage,
            Completeness::Complete,
        )?;
        self.proof = Some(ImageProof {
            reference: reference.clone(),
            rowid,
            digest,
        });
        self.revalidate(&reference)?;
        Ok(vec![Discovered {
            reference,
            stored_bytes: None,
        }])
    }

    fn revalidate(&self, reference: &Reference) -> Result<(), Error> {
        reference.validate_owner(&self.owner)?;
        let proof = self
            .proof
            .as_ref()
            .filter(|proof| &proof.reference == reference)
            .ok_or_else(|| Error::new(Stage::Revalidation, Failure::InvalidReference))?;
        self.messages
            .revalidate(reference.message())
            .map_err(|error| message_failure(error, Stage::Revalidation))?;
        self.resource_pin
            .verify()
            .map_err(|_| Error::new(Stage::Revalidation, Failure::StaleEvidence))?;
        match self
            .resources
            .lookup(&self.identity)
            .map_err(|_| Error::new(Stage::Revalidation, Failure::Unavailable))?
        {
            ResourceLookup::Found(rowid, digest)
                if rowid == proof.rowid && digest == proof.digest =>
            {
                Ok(())
            }
            ResourceLookup::Ambiguous => Err(Error::new(Stage::Revalidation, Failure::Ambiguous)),
            _ => Err(Error::new(Stage::Revalidation, Failure::StaleEvidence)),
        }
    }
}
