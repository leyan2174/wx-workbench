//! Resource type encoding for the explicit legacy attachment protocol.
use crate::attachment::AttachmentKind;

pub(crate) fn resource_type(kind: AttachmentKind) -> i64 {
    match kind {
        AttachmentKind::Image => 3,
        AttachmentKind::Voice => 34,
        AttachmentKind::Video => 43,
        AttachmentKind::File => 49,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_resource_encoding_preserves_all_protocol_kinds() {
        for (kind, expected) in [
            (AttachmentKind::Image, 3),
            (AttachmentKind::Voice, 34),
            (AttachmentKind::Video, 43),
            (AttachmentKind::File, 49),
        ] {
            assert_eq!(resource_type(kind), expected);
        }
    }
}
