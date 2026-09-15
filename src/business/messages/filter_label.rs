//! User-facing filter intent, without storage or wire codes.
use super::Kind;

/// Narrow labels supplement Kind. Link/File remain distinct intents even when
/// a legacy protocol cannot distinguish them. The other labels are not Structured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterLabel {
    Kind(Kind),
    Sticker,
    Location,
    ContactCard,
    Link,
    File,
}

impl FilterLabel {
    /// Canonical labels only; aliases and case policy belong to transports.
    pub fn parse(label: &str) -> Option<Self> {
        Some(match label {
            "text" => Self::Kind(Kind::Text),
            "image" => Self::Kind(Kind::Image),
            "voice" => Self::Kind(Kind::Voice),
            "video" => Self::Kind(Kind::Video),
            "call" => Self::Kind(Kind::Call),
            "system" => Self::Kind(Kind::System),
            "app" => Self::Kind(Kind::Structured),
            "sticker" => Self::Sticker,
            "location" => Self::Location,
            "namecard" => Self::ContactCard,
            "link" => Self::Link,
            "file" => Self::File,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_reuse_kinds_without_claiming_missing_or_narrower_kinds() {
        assert_eq!(
            FilterLabel::parse("video"),
            Some(FilterLabel::Kind(Kind::Video))
        );
        assert_ne!(FilterLabel::parse("file"), FilterLabel::parse("app"));
        assert_ne!(FilterLabel::parse("file"), FilterLabel::parse("link"));
        for label in ["sticker", "location", "namecard"] {
            assert!(!matches!(
                FilterLabel::parse(label),
                Some(FilterLabel::Kind(_))
            ));
        }
        for label in ["TEXT", " text ", "emoji", "voip"] {
            assert_eq!(FilterLabel::parse(label), None);
        }
    }
}
