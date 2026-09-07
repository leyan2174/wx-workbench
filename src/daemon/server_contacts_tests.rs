use super::*;
use rusqlite::Connection;
use serde_json::json;
use std::{collections::HashMap, fs, path::Path, time::UNIX_EPOCH};

async fn seeded(root: &Path, key: &str, marker: &str) -> (DbCache, std::path::PathBuf) {
    let source = root.join("db_storage/contact/contact.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"synthetic encrypted source").unwrap();
    let cached = root.join("cache/contact.db");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    let conn = Connection::open(&cached).unwrap();
    conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,alias TEXT,description TEXT,phone TEXT,local_type INTEGER);
        INSERT INTO contact VALUES('wxid_1','HiddenNick','VisibleRemark','alias','memo','123',1),
        ('group@chatroom','Group','','','','',1),('gh_public','Public','','','','',1),
        ('wxid_1','Duplicate','','','','',1),('excluded','HiddenNick','','','','',3);").unwrap();
    conn.execute("INSERT INTO contact VALUES(?1,'','','','','',1)", [marker])
        .unwrap();
    drop(conn);
    let mt = fs::metadata(&source)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
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
        md5_to_uname: HashMap::new(),
        msg_db_keys: vec![],
        biz_msg_db_keys: vec![],
        verify_flags: HashMap::new(),
    }))
}

#[tokio::test]
async fn contacts_legacy_dispatch_reads_account_rows_and_preserves_cli() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (db_a, path_a) = seeded(a.path(), "contact/contact.db", "account_a").await;
    let (db_b, path_b) = seeded(b.path(), "contact\\contact.db", "account_b").await;
    let before_a = fs::read(&path_a).unwrap();
    let before_b = fs::read(&path_b).unwrap();
    let names = names();
    for (db, own, other) in [
        (&db_a, "account_a", "account_b"),
        (&db_b, "account_b", "account_a"),
    ] {
        let request = crate::mcp::protocol::route("get_contacts", &json!({})).unwrap();
        let response = dispatch(request, db, &names).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["total"], 5);
        let rows = response.data["contacts"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r["username"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["wxid_1", "group@chatroom", "gh_public", "wxid_1", own]
        );
        assert!(!response.data.to_string().contains(other));
        assert_eq!(
            rows[0],
            json!({"username":"wxid_1","nick_name":"HiddenNick","remark":"VisibleRemark","alias":"alias","description":"memo","phone":"123","display":"VisibleRemark"})
        );
        let request =
            crate::mcp::protocol::route("get_contacts", &json!({"query":"hiddenNICK"})).unwrap();
        let response = dispatch(request, db, &names).await;
        assert!(response.ok);
        assert_eq!(response.data["total"], 1);
        let cli: Request = serde_json::from_value(json!({"cmd":"contacts"})).unwrap();
        let response = dispatch(cli, db, &names).await;
        assert!(response.ok);
        assert_eq!(response.data["total"], 1);
        assert_eq!(response.data["contacts"][0]["username"], "wxid_1");
        let cli: Request =
            serde_json::from_value(json!({"cmd":"contacts","query":"hiddenNICK"})).unwrap();
        assert_eq!(dispatch(cli, db, &names).await.data["total"], 0);
    }
    assert_eq!(fs::read(path_a).unwrap(), before_a);
    assert_eq!(fs::read(path_b).unwrap(), before_b);
}

#[tokio::test]
async fn contacts_legacy_missing_database_does_not_fall_back_to_names() {
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
    assert!(!response.ok);
    assert!(response
        .error
        .unwrap()
        .contains("contact database unavailable"));
    assert!(!root.path().join("missing").exists());
}
