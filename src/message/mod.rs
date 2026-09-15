//! Message parsing shared by query and export adapters.
pub mod export;
pub mod group_content;
pub mod identity;
pub mod location;
pub mod structured_message;
pub mod summary;
pub mod transfer;
pub(crate) mod xml;

pub use group_content::split_group_content;
