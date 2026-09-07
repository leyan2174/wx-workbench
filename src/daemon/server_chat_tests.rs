use super::*;
use std::collections::HashMap;

#[tokio::test]
async fn exact_chat_resolution_needs_no_message_or_media_database() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("missing-source");
    let db = DbCache::with_dirs(
        source.clone(),
        root.path().join("cache"),
        root.path().join("mtimes.json"),
        HashMap::new(),
    )
    .await
    .unwrap();
    let names = tokio::sync::RwLock::new(Arc::new(Names {
        map: HashMap::from([
            ("wxid_a".into(), "Shared".into()),
            ("wxid_b".into(), "Shared".into()),
            ("room@chatroom".into(), "Unique".into()),
        ]),
        md5_to_uname: HashMap::new(),
        msg_db_keys: vec![],
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    }));
    for (chat, expected) in [
        ("wxid_a", Some("wxid_a")),
        ("Unique", Some("room@chatroom")),
        ("Shared", None),
        ("unique", None),
        ("missing", None),
        ("", None),
    ] {
        let request = serde_json::from_value(serde_json::json!({
            "cmd": "resolve_chat", "chat": chat
        }))
        .unwrap();
        let response = dispatch(request, &db, &names).await;
        assert_eq!(response.ok, expected.is_some());
        if let Some(username) = expected {
            assert_eq!(response.data, serde_json::json!({"username":username}));
        } else {
            assert_eq!(
                response.error.as_deref(),
                Some("Chat has no unique exact match")
            );
        }
    }
    assert!(!source.exists());
}
