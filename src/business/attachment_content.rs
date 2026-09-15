//! Storage-independent decoded attachment content. No private XML or source coordinates.
use serde::Serialize;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Image,
    Voice,
    Video,
    Text,
    MetadataOnly,
    Emoticon,
}

/// Decoded content, independent of message storage identities and format codes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentContent {
    pub kind: Kind,
    pub item_index: Option<usize>,
    pub item_count: Option<usize>,
    pub title: String,
    pub extension: String,
    pub expected_size: Option<u64>,
    pub expected_md5: Option<String>,
    pub sender: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    File,
    Record,
}

/// Select a standalone attachment or an item in a forwarded collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    File,
    RecordItem(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedMedia {
    pub kind: NamedKind,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKind {
    Video,
    Emoticon,
}
