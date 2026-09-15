//! Media discovery and association rules; execution authority stays with the host.
use super::messages::{Completeness, MessageRef};
use std::{
    fmt,
    sync::{Arc, Weak},
};

/// Valid only while the creating message/media read instances remain valid.
#[derive(Clone, Debug)]
pub struct Reference {
    read_scope: Weak<()>,
    message: MessageRef,
    kind: Kind,
    item_index: Option<u32>,
    association: AssociationPolicy,
    completeness: Completeness,
}

impl Reference {
    pub(crate) fn new(
        owner: &Arc<()>,
        message: MessageRef,
        kind: Kind,
        item_index: Option<u32>,
        association: AssociationPolicy,
        completeness: Completeness,
    ) -> Result<Self, Error> {
        if completeness != Completeness::Complete {
            return Err(Error::new(Stage::Discovery, Failure::IncompleteSources));
        }
        if message.evidence().is_expired() {
            return Err(Error::new(Stage::Discovery, Failure::StaleEvidence));
        }
        Ok(Self {
            read_scope: Arc::downgrade(owner),
            message,
            kind,
            item_index,
            association,
            completeness,
        })
    }
    pub fn message(&self) -> &MessageRef {
        &self.message
    }
    #[cfg(test)]
    pub fn association(&self) -> AssociationPolicy {
        self.association
    }
    #[cfg(test)]
    pub fn completeness(&self) -> Completeness {
        self.completeness
    }
    pub(crate) fn validate_owner(&self, owner: &Arc<()>) -> Result<(), Error> {
        if self.message.evidence().is_expired()
            || !self
                .read_scope
                .upgrade()
                .is_some_and(|scope| Arc::ptr_eq(&scope, owner))
        {
            return Err(Error::new(Stage::Revalidation, Failure::StaleEvidence));
        }
        Ok(())
    }
}

impl PartialEq for Reference {
    fn eq(&self, other: &Self) -> bool {
        self.read_scope.ptr_eq(&other.read_scope)
            && self.message == other.message
            && self.kind == other.kind
            && self.item_index == other.item_index
            && self.association == other.association
            && self.completeness == other.completeness
    }
}
impl Eq for Reference {}

#[derive(Clone, Debug)]
pub struct Discovered {
    pub reference: Reference,
    /// Stored bytes only. Unknown is not zero and does not imply decoded size.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Unknown stored size remains distinct from decoded or exported byte counts"
        )
    )]
    pub stored_bytes: Option<u64>,
}

/// Discovery never decodes, starts a process, publishes, or uploads media.
/// The adapter retains private resource coordinates and checks them on reuse.
pub trait Source {
    fn discover(&mut self, message: &MessageRef, kind: Kind) -> Result<Vec<Discovered>, Error>;
    fn revalidate(&self, reference: &Reference) -> Result<(), Error>;
}

pub fn message_error(error: super::messages::Error, stage: Stage) -> Error {
    use super::messages::Error as MessageError;
    let failure = match error {
        MessageError::NotFound => Failure::NotFound,
        MessageError::Ambiguous => Failure::Ambiguous,
        MessageError::Expired => Failure::StaleEvidence,
        MessageError::Unsupported => Failure::Unsupported,
        MessageError::Unavailable => Failure::IncompleteSources,
        MessageError::InvalidData => Failure::InvalidReference,
        MessageError::Limit => Failure::LimitExceeded,
    };
    Error::new(stage, failure)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Discovery retains voice requests even while the strict production resolver supports images only"
        )
    )]
    Voice,
    #[expect(
        dead_code,
        reason = "Unsupported video discovery remains a typed request, not an image fallback"
    )]
    Video,
    #[expect(
        dead_code,
        reason = "Unsupported file discovery remains a typed request, not an image fallback"
    )]
    File,
    #[expect(
        dead_code,
        reason = "Message emoticon discovery is distinct from the materialized emoticon catalog"
    )]
    Emoticon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Discovery,
    Association,
    Revalidation,
    Decode,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Transcription is a distinct stage; current ASR execution uses the voice domain contract"
        )
    )]
    Transcription,
    Publication,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    InvalidReference,
    NotFound,
    IncompleteSources,
    Ambiguous,
    ConflictingEvidence,
    StaleEvidence,
    UnsafeSource,
    Unsupported,
    InvalidMaterial,
    LimitExceeded,
    Refused,
    #[expect(
        dead_code,
        reason = "Cancellation remains distinct from unavailable media; execution hosts currently report it before this boundary"
    )]
    Cancelled,
    #[expect(
        dead_code,
        reason = "Deadline exhaustion remains distinct from unavailable media; execution hosts currently report it before this boundary"
    )]
    DeadlineExceeded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub stage: Stage,
    pub failure: Failure,
}

impl Error {
    pub const fn new(stage: Stage, failure: Failure) -> Self {
        Self { stage, failure }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Only fixed classifications cross the business boundary, never source errors.
        write!(f, "Media {:?}: {:?}", self.stage, self.failure)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod stage_label_tests {
    use super::{Error, Failure, Stage};

    #[test]
    fn download_is_distinct_and_existing_stage_labels_are_unchanged() {
        for (stage, label) in [
            (Stage::Discovery, "Discovery"),
            (Stage::Association, "Association"),
            (Stage::Revalidation, "Revalidation"),
            (Stage::Decode, "Decode"),
            (Stage::Transcription, "Transcription"),
            (Stage::Publication, "Publication"),
            (Stage::Download, "Download"),
        ] {
            assert_eq!(
                Error::new(stage, Failure::Unavailable).to_string(),
                format!("Media {label}: Unavailable"),
            );
        }
    }
}

/// Distinct entry contracts, not permission to retry a failed strict association.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssociationPolicy {
    StrictMessage,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Explicit legacy association cannot be confused with strict message evidence or authorize strict fallback"
        )
    )]
    ExplicitLegacyMediaId,
}

/// A server id proves a voice join only together with exact conversation and time.
/// Neither message local_id nor media local_id participates in this equality.
pub fn verify_voice_join(
    message_server_id: i64,
    message_time: i64,
    expected_media_chat_id: i64,
    media_server_id: i64,
    media_time: i64,
    media_chat_id: i64,
) -> Result<(), Error> {
    if message_server_id == 0
        || message_server_id != media_server_id
        || message_time != media_time
        || expected_media_chat_id != media_chat_id
    {
        return Err(Error::new(Stage::Association, Failure::ConflictingEvidence));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_join_requires_all_evidence_not_local_ids_or_time_alone() {
        assert!(verify_voice_join(70, 123, 9, 70, 123, 9).is_ok());
        for values in [
            (0, 123, 9, 0, 123, 9),
            (70, 123, 9, 71, 123, 9),
            (70, 123, 9, 70, 124, 9),
            (70, 123, 9, 70, 123, 10),
        ] {
            assert_eq!(
                verify_voice_join(values.0, values.1, values.2, values.3, values.4, values.5),
                Err(Error::new(Stage::Association, Failure::ConflictingEvidence)),
            );
        }
    }

    #[test]
    fn incomplete_ambiguous_and_stale_results_are_not_absence() {
        for failure in [
            Failure::IncompleteSources,
            Failure::Ambiguous,
            Failure::StaleEvidence,
        ] {
            assert_ne!(
                Error::new(Stage::Discovery, failure).failure,
                Failure::NotFound
            );
        }
    }
}
