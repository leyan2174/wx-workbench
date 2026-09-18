#![cfg(test)]

use super::*;
use crate::daemon::query::encrypted_cache;
use serde_json::json;
use std::{collections::HashMap, fs, path::Path};

async fn seeded(root: &Path, key: &str, marker: &str) -> (DbCache, std::path::PathBuf) {
    let source = root.join("db_storage/contact/contact.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let cached = root.join("cache/contact.db");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    let conn = encrypted_cache::sqlite(&cached);
    conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,alias TEXT,description TEXT,phone TEXT,local_type INTEGER);
        INSERT INTO contact VALUES('wxid_1','HiddenNick','VisibleRemark','alias','memo','123',1),
        ('group@chatroom','Group','','','','',1),('gh_public','Public','','','','',1),
        ('wxid_1','Duplicate','','','','',1),('excluded','HiddenNick','','','','',3);").unwrap();
    conn.execute("INSERT INTO contact VALUES(?1,'','','','','',1)", [marker])
        .unwrap();
    drop(conn);
    let mt = encrypted_cache::seed(&cached, &source);
    let mtime = root.join("cache/mtimes.json");
    fs::write(
        &mtime,
        serde_json::to_vec(&json!({key:{"db_mt":mt,"wal_mt":0,"path":cached}})).unwrap(),
    )
    .unwrap();
    let db = DbCache::with_dirs(
        root.join("db_storage"),
        root.join("cache"),
        mtime,
        HashMap::from([(key.to_owned(), "11".repeat(32))]),
    )
    .await
    .unwrap();
    (db, cached)
}

fn names() -> tokio::sync::RwLock<Arc<Names>> {
    tokio::sync::RwLock::new(Arc::new(Names {
        map: HashMap::from([
            ("wxid_1".into(), "VisibleRemark".into()),
            ("group@chatroom".into(), "Group".into()),
            ("gh_public".into(), "Public".into()),
        ]),
        msg_db_keys: vec![],
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    }))
}

#[tokio::test]
async fn decode_image_dispatch_uses_distinct_redacted_export_failure_code() {
    let root = tempfile::tempdir().unwrap();
    let (db, cached) = seeded(root.path(), "contact/contact.db", "synthetic").await;
    let before = fs::read(&cached).unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let names = names();
    let missing = dispatch(
        Request::DecodeImage {
            chat: "PRIVATE_PEER".into(),
            local_id: 7,
            create_time: 123,
            output_root: output.to_str().unwrap().into(),
        },
        &db,
        &names,
    )
    .await;
    assert!(missing.ok);
    assert_eq!(missing.data["exit_code"], 1);

    // A protected output root triggers a real export error before chat lookup.
    let snapshot = names.read().await.clone();
    let error = crate::daemon::query::mcp_image::q_decode_image_for_host(
        &db,
        &snapshot,
        "PRIVATE_PEER",
        7,
        123,
        db.db_dir(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "image output conflicts with protected input"
    );
    let response = dispatch(
        Request::DecodeImage {
            chat: "PRIVATE_PEER".into(),
            local_id: 7,
            create_time: 123,
            output_root: db.db_dir().to_str().unwrap().into(),
        },
        &db,
        &names,
    )
    .await;
    assert!(response.ok);
    assert!(response.error.is_none());
    assert_eq!(
        response.data,
        json!({"exit_code":3,"status":"error","message":"Image export failed"})
    );
    assert_eq!(fs::read_dir(output).unwrap().count(), 0);
    assert_eq!(fs::read(cached).unwrap(), before);
}

#[tokio::test]
async fn contacts_dispatch_uses_one_account_snapshot_for_mcp_and_ipc() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (db_a, path_a) = seeded(a.path(), "contact/contact.db", "account_a").await;
    let (db_b, path_b) = seeded(b.path(), "contact\\contact.db", "account_b").await;
    let before_a = fs::read(&path_a).unwrap();
    let before_b = fs::read(&path_b).unwrap();
    for (db, own, other) in [
        (&db_a, "account_a", "account_b"),
        (&db_b, "account_b", "account_a"),
    ] {
        let names = tokio::sync::RwLock::new(Arc::new(Names {
            map: HashMap::from([
                ("wxid_1".into(), "VisibleRemark".into()),
                (own.into(), own.into()),
                ("group@chatroom".into(), "Group".into()),
                ("gh_public".into(), "Public".into()),
            ]),
            msg_db_keys: vec![],
            biz_msg_db_keys: vec![],
            verify_flags: HashMap::new(),
        }));
        let request = crate::mcp::protocol::route("get_contacts", &json!({})).unwrap();
        let response = dispatch(request, db, &names).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["total"], 2);
        let rows = response.data["contacts"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r["username"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["wxid_1", own]
        );
        assert!(!response.data.to_string().contains(other));
        assert_eq!(
            rows[0],
            json!({"username":"wxid_1","display":"VisibleRemark"})
        );
        let expected = response.data.clone();
        let request =
            crate::mcp::protocol::route("get_contacts", &json!({"query":"hiddenNICK"})).unwrap();
        let response = dispatch(request, db, &names).await;
        assert!(response.ok);
        assert_eq!(response.data["total"], 0);
        let cli: Request = serde_json::from_value(json!({"cmd":"contacts"})).unwrap();
        let response = dispatch(cli, db, &names).await;
        assert!(response.ok);
        assert_eq!(response.data, expected);
        assert_eq!(response.data["contacts"][0]["username"], "wxid_1");
        let cli: Request =
            serde_json::from_value(json!({"cmd":"contacts","query":"hiddenNICK"})).unwrap();
        assert_eq!(dispatch(cli, db, &names).await.data["total"], 0);
    }
    assert_eq!(fs::read(path_a).unwrap(), before_a);
    assert_eq!(fs::read(path_b).unwrap(), before_b);
}

#[tokio::test]
async fn contacts_use_materialized_names_without_reopening_databases_but_reject_empty_cache() {
    let root = tempfile::tempdir().unwrap();
    let db = DbCache::with_dirs(
        root.path().join("missing"),
        root.path().join("cache"),
        root.path().join("mtimes.json"),
        HashMap::new(),
    )
    .await
    .unwrap();
    let response = dispatch(
        crate::mcp::protocol::route("get_contacts", &json!({})).unwrap(),
        &db,
        &names(),
    )
    .await;
    assert!(response.ok);
    assert_eq!(
        response.data,
        json!({"contacts":[{"username":"wxid_1","display":"VisibleRemark"}],"total":1})
    );
    let empty = tokio::sync::RwLock::new(Arc::new(Names {
        map: HashMap::new(),
        msg_db_keys: vec![],
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    }));
    let response = dispatch(
        crate::mcp::protocol::route("get_contacts", &json!({})).unwrap(),
        &db,
        &empty,
    )
    .await;
    assert!(!response.ok);
    assert!(response.error.unwrap().contains("wx daemon reload"));
    assert!(!root.path().join("missing").exists());
}
