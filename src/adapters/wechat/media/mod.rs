//! Account-bound media evidence. Paths and resource coordinates stay in this adapter.
pub mod resource;
pub mod voice;
pub mod voice_catalog;
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
