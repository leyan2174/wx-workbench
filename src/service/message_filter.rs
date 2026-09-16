//! Compatibility projection for existing numeric queries, not a storage classifier.
use crate::business::messages::{filter_label::FilterLabel, Kind};

fn canonical_label(value: &str) -> Option<FilterLabel> {
    Some(match value {
        "text" => FilterLabel::Kind(Kind::Text),
        "image" => FilterLabel::Kind(Kind::Image),
        "voice" => FilterLabel::Kind(Kind::Voice),
        "video" => FilterLabel::Kind(Kind::Video),
        "call" => FilterLabel::Kind(Kind::Call),
        "system" => FilterLabel::Kind(Kind::System),
        "sticker" => FilterLabel::Sticker,
        "location" => FilterLabel::Location,
        "link" => FilterLabel::Link,
        "file" => FilterLabel::File,
        _ => return None,
    })
}

fn cli_label(value: &str) -> Option<FilterLabel> {
    canonical_label(value)
}

fn mcp_label(value: &str) -> Option<FilterLabel> {
    let normalized = value.trim().to_ascii_lowercase();
    canonical_label(&normalized)
}

/// Preserve Video=43 (not 62) and System=10000 (not 10002).
/// Link/File intentionally project to the broad app selector. The read adapter
/// alone interprets base versus packed exact numbers.
pub fn legacy_wire_type(label: FilterLabel) -> Option<i64> {
    Some(match label {
        FilterLabel::Kind(Kind::Text) => 1,
        FilterLabel::Kind(Kind::Image) => 3,
        FilterLabel::Kind(Kind::Voice) => 34,
        FilterLabel::Kind(Kind::Video) => 43,
        FilterLabel::Sticker => 47,
        FilterLabel::Location => 48,
        FilterLabel::Kind(Kind::Structured) | FilterLabel::Link | FilterLabel::File => 49,
        FilterLabel::Kind(Kind::Call) => 50,
        FilterLabel::Kind(Kind::System) => 10000,
        FilterLabel::Kind(Kind::Unknown) => return None,
    })
}

pub fn cli_type(value: &str) -> Option<i64> {
    cli_label(value).and_then(legacy_wire_type)
}

pub fn mcp_type(value: &str) -> Option<i64> {
    mcp_label(value).and_then(legacy_wire_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_allowed_values_and_numbers_are_unchanged() {
        for (label, number) in [
            ("text", 1),
            ("image", 3),
            ("voice", 34),
            ("video", 43),
            ("sticker", 47),
            ("location", 48),
            ("link", 49),
            ("file", 49),
            ("call", 50),
            ("system", 10000),
        ] {
            assert_eq!(cli_type(label), Some(number));
            assert_eq!(cli_type(&label.to_ascii_uppercase()), None);
            assert_eq!(cli_type(&format!(" {label} ")), None);
        }
        for label in ["app", "namecard", "emoji", "voip", "", "49"] {
            assert_eq!(cli_type(label), None);
        }
    }

    #[test]
    fn mcp_uses_formal_business_labels_without_compatibility_words() {
        for (label, number) in [
            ("text", 1),
            ("image", 3),
            ("voice", 34),
            ("video", 43),
            ("sticker", 47),
            ("location", 48),
            ("link", 49),
            ("file", 49),
            ("call", 50),
            ("system", 10000),
        ] {
            assert_eq!(
                mcp_type(&format!(" {} ", label.to_ascii_uppercase())),
                Some(number)
            );
        }
        for label in ["emoji", "voip", "app", "namecard", "", "49"] {
            assert_eq!(mcp_type(label), None);
        }
        assert_eq!(cli_label("sticker"), mcp_label(" STICKER "));
        assert_eq!(cli_label("call"), mcp_label(" CALL "));
        assert_eq!(cli_type("file"), mcp_type(" FILE "));
        assert_eq!(legacy_wire_type(FilterLabel::Kind(Kind::Unknown)), None);
    }
}
