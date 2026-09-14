//! Account message reads and semantic projections share one validated snapshot implementation.
pub mod catalog;
mod directory_selection;
pub(crate) mod legacy;
mod projection;
pub mod read;
pub mod sessions;
pub mod sources;
pub mod statistics;
#[cfg(test)]
use projection::call_event;
pub use projection::semantic_kind;
pub use read::{
    DetachedContent, LegacyReadPolicy, RawMessage, Snapshot, SourceFile, StoredContent,
    StoredScalar, MAX_DECODED_BYTES, MAX_STORED_BYTES,
};
#[cfg(test)]
mod tests;
