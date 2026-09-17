use super::*;
use serde_json::json;

#[test]
fn synthetic_golden_cases() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/chat-merge-golden.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let old = case["existing"].clone();
        let new = case["incoming"].clone();
        let result = merge_chat_json(&old, &new);
        let actual = match result {
            Ok(result) => json!({"ok": result}),
            Err(error) => json!({"error": error}),
        };
        assert_eq!(actual, case["expected"], "case: {}", case["name"]);
        assert_eq!(old, case["existing"]);
        assert_eq!(new, case["incoming"]);
    }
}

#[test]
fn validates_shapes_and_values_without_panicking() {
    let good = json!({"username":"synthetic", "messages":[]});
    for bad in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"username":" ","messages":[]}),
        json!({"username":"synthetic","messages":null}),
        json!({"username":"synthetic","messages":[null]}),
    ] {
        assert!(matches!(
            merge_chat_json(&good, &bad),
            Err(MergeError::InvalidInput { .. })
        ));
        assert!(matches!(
            merge_chat_json(&bad, &good),
            Err(MergeError::InvalidInput { .. })
        ));
    }
    for (field, values) in [
        (
            "local_id",
            vec![Value::Null, json!(true), json!(1.5), json!(""), json!({})],
        ),
        (
            "source",
            vec![json!(0), json!(false), json!(" "), json!({})],
        ),
        (
            "timestamp",
            vec![json!(false), json!("123"), json!(1.5), json!([])],
        ),
    ] {
        for value in values {
            let mut bad = json!({"username":"synthetic","messages":[{"local_id":1,"source":"a","timestamp":0}]});
            bad["messages"][0][field] = value;
            assert!(
                matches!(
                    merge_chat_json(&good, &bad),
                    Err(MergeError::InvalidInput { .. })
                ),
                "{bad}"
            );
        }
    }
}

#[test]
fn known_source_merge_is_idempotent_and_counts_balance() {
    let old = json!({"username":"synthetic","messages":[]});
    let batch = json!({"username":"synthetic","messages":[
        {"local_id":1,"source":"a","timestamp":3},
        {"local_id":1,"source":"b","timestamp":2},
        {"local_id":1,"source":"a","timestamp":3}
    ]});
    let once = merge_chat_json(&old, &batch).unwrap();
    let twice = merge_chat_json(&once.document, &batch).unwrap();
    assert_eq!(once.document, twice.document);
    assert_eq!(once.report.added, 2);
    assert_eq!(once.report.duplicates, 1);
    assert_eq!(twice.report.added, 0);
    assert_eq!(twice.report.duplicates, 3);
    assert_eq!(
        twice.report.retained + twice.report.added,
        twice.document["messages"].as_array().unwrap().len()
    );
}

#[test]
fn integer_extremes_sort_exactly_and_string_ids_stay_distinct() {
    let old = json!({"username":"synthetic","messages":[]});
    let batch = json!({"username":"synthetic","messages":[
        {"local_id":1,"source":"a","timestamp":u64::MAX},
        {"local_id":"1","source":"a","timestamp":i64::MIN},
        {"local_id":2,"source":"a","timestamp":9007199254740993_u64},
        {"local_id":3,"source":"a","timestamp":9007199254740992_u64}
    ]});
    let merged = merge_chat_json(&old, &batch).unwrap();
    let ids: Vec<_> = merged.document["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["local_id"].clone())
        .collect();
    assert_eq!(ids, vec![json!("1"), json!(3), json!(2), json!(1)]);
}

#[test]
fn no_source_collision_is_atomic_even_when_payload_is_identical() {
    let doc = json!({"username":"synthetic","messages":[{"local_id":1,"timestamp":0,"annotation":"synthetic"}]});
    match merge_chat_json(&doc, &doc) {
        Err(MergeError::Ambiguous { report, conflicts }) => {
            assert_eq!(
                report,
                MergeReport {
                    added: 0,
                    retained: 1,
                    duplicates: 0,
                    ambiguous: 1
                }
            );
            assert_eq!(conflicts[0].locations.len(), 2);
        }
        other => panic!("unexpected: {other:?}"),
    }
}
