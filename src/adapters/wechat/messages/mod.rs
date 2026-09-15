//! Account message reads and semantic projections share one validated snapshot implementation.
pub mod catalog;
mod directory_selection;
pub mod export_content;
pub mod inventory;
pub(crate) mod legacy;
pub mod location;
pub mod probe;
mod projection;
pub use projection::pages;
pub mod read;
pub mod reply;
pub mod reply_read;
pub mod sessions;
pub mod sources;
pub mod statistics;
pub mod summary;
pub mod transfer;
#[cfg(test)]
use projection::call_event;
pub use projection::semantic_kind;
pub use read::{
    DetachedContent, LegacyReadPolicy, RawMessage, Snapshot, SourceFile, StoredContent,
    StoredScalar, MAX_DECODED_BYTES, MAX_STORED_BYTES,
};
#[cfg(test)]
mod tests;
