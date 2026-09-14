//! Account message reads and semantic projections share one validated snapshot implementation.
pub(crate) mod legacy;
mod projection;
pub mod read;
pub mod sessions;
pub use projection::{call_event, semantic_kind};
pub use read::{
    DetachedContent, LegacyReadPolicy, RawMessage, Snapshot, SourceFile, StoredContent,
    StoredScalar, Stream, MAX_DECODED_BYTES, MAX_STORED_BYTES,
};
#[cfg(test)]
mod tests;
