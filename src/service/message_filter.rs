//! Compatibility projection for existing numeric queries, not a storage classifier.
use crate::business::messages::{filter_label::FilterLabel, Kind};

fn cli_label(value: &str) -> Option<FilterLabel> {
    match value {
        "app" | "namecard" => None,
        _ => FilterLabel::parse(value),
    }
}

fn mcp_label(value: &str) -> Option<FilterLabel> {
    let normalized = value.trim().to_ascii_lowercase();
    FilterLabel::parse(match normalized.as_str() {
        "emoji" => "sticker",
        "voip" => "call",
        "sticker" | "call" | "link" => return None,
        other => other,
    })
}

/// Preserve Video=43 (not 62) and System=10000 (not 10002).
/// Link/File intentionally project to the broad app selector. The read adapter
/// alone interprets base versus packed exact numbers.
pub fn legacy_wire_type(label: FilterLabel) -> Option<i64> {
    Some(match label {
        FilterLabel::Kind(Kind::Text) => 1,
        FilterLabel::Kind(Kind::Image) => 3,
        FilterLabel::Kind(Kind::Voice) => 34,
        FilterLabel::ContactCard => 42,
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
    fn mcp_aliases_case_and_intent_differences_are_preserved() {
        for (label, number) in [
            ("text", 1),
            ("image", 3),
            ("voice", 34),
            ("namecard", 42),
            ("video", 43),
            ("emoji", 47),
            ("location", 48),
            ("app", 49),
            ("file", 49),
            ("voip", 50),
            ("system", 10000),
        ] {
            assert_eq!(
                mcp_type(&format!(" {} ", label.to_ascii_uppercase())),
                Some(number)
            );
        }
        for label in ["sticker", "call", "link", "", "49"] {
            assert_eq!(mcp_type(label), None);
        }
        assert_eq!(cli_label("sticker"), mcp_label("EMOJI"));
        assert_eq!(cli_label("call"), mcp_label(" VOIP "));
        assert_ne!(cli_label("file"), mcp_label("app"));
        assert_eq!(cli_type("file"), mcp_type("app"));
        assert_eq!(legacy_wire_type(FilterLabel::Kind(Kind::Unknown)), None);
    }
}
