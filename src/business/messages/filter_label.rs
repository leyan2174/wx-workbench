//! User-facing filter intent, without storage or wire codes.
use super::Kind;

/// Narrow labels supplement Kind. Link/File remain distinct intents even when
/// the numeric protocol filter cannot distinguish them. The other labels are not Structured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterLabel {
    Kind(Kind),
    Sticker,
    Location,
    Link,
    File,
}
