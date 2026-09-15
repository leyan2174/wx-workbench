use super::*;
use serde_json::json;

#[test]
fn business_selectors_keep_the_exact_cli_allowed_values_and_wire() {
    for (label, kind, number) in [
        ("text", FavoriteKind::Text, 1),
        ("image", FavoriteKind::Image, 2),
        ("article", FavoriteKind::Article, 5),
        ("card", FavoriteKind::ContactCard, 19),
        ("video", FavoriteKind::Video, 20),
    ] {
        assert_eq!(parse_fav_type(label).unwrap(), kind);
        assert_eq!(
            serde_json::to_value(request(7, Some(label), Some("needle".into())).unwrap()).unwrap(),
            json!({"cmd":"favorites","limit":7,"fav_type":number,"query":"needle"})
        );
        assert!(request(7, Some(&label.to_ascii_uppercase()), None).is_err());
        assert!(request(7, Some(&format!(" {label} ")), None).is_err());
    }
    for label in ["", "5", "app", "other", "contactcard"] {
        assert!(request(7, Some(label), None).is_err());
    }
    assert_eq!(
        serde_json::to_value(request(7, None, None).unwrap()).unwrap(),
        json!({"cmd":"favorites","limit":7})
    );
}

#[test]
fn missing_or_malformed_page_metadata_is_not_empty_success() {
    for data in [
        json!({}),
        json!({"count":0,"has_more":false}),
        json!({"count":0,"has_more":false,"items":null}),
        json!({"count":0,"has_more":false,"items":{}}),
        json!({"count":0,"items":[]}),
        json!({"count":0,"has_more":"false","items":[]}),
        json!({"has_more":false,"items":[]}),
        json!({"count":1,"has_more":false,"items":[]}),
        json!({"count":-1,"has_more":false,"items":[]}),
        json!({"count":1,"has_more":false,"items":[null]}),
    ] {
        assert!(page(&data).is_err(), "{data}");
    }
}

#[test]
fn continuation_is_visible_without_changing_legacy_json_items() {
    let items = json!([{"id":7,"type":"article","type_num":5,"preview":"kept","url":"https://example.test"}]);
    for has_more in [false, true] {
        let data = json!({"count":1,"items":items,"has_more":has_more});
        let (projected, more) = page(&data).unwrap();
        assert_eq!(projected, &items);
        assert_eq!(
            serde_json::to_string(projected).unwrap(),
            serde_json::to_string(&items).unwrap()
        );
        assert_eq!(continuation_notice(more).is_some(), has_more);
    }
    // limit=0 can legitimately return an empty first page with more matches.
    let data = json!({"count":0,"items":[],"has_more":true});
    let (_, more) = page(&data).unwrap();
    assert!(continuation_notice(more).unwrap().contains("--limit"));
}
