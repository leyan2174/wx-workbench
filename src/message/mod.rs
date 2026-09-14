//! Message parsing shared by query and export adapters.
pub mod export;
pub mod export_content;
pub mod group_content;
pub mod identity;
pub mod location;
pub mod summary;
pub mod transfer;
pub(crate) mod xml;

pub use group_content::split_group_content;
