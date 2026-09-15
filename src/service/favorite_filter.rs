//! Existing favorite request numbers belong to this compatibility boundary.
use crate::business::favorites::FavoriteKind;

pub fn legacy_wire_type(kind: FavoriteKind) -> Option<i64> {
    Some(match kind {
        FavoriteKind::Text => 1,
        FavoriteKind::Image => 2,
        FavoriteKind::Article => 5,
        FavoriteKind::ContactCard => 19,
        FavoriteKind::Video => 20,
        FavoriteKind::Other => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_business_kinds_preserve_legacy_request_numbers() {
        for (kind, number) in [
            (FavoriteKind::Text, 1),
            (FavoriteKind::Image, 2),
            (FavoriteKind::Article, 5),
            (FavoriteKind::ContactCard, 19),
            (FavoriteKind::Video, 20),
        ] {
            assert_eq!(legacy_wire_type(kind), Some(number));
        }
        assert_eq!(legacy_wire_type(FavoriteKind::Other), None);
    }
}
