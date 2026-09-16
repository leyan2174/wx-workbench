use super::*;
use super::{encrypted_cache::encrypted_sqlite, mcp_contacts};
use std::{collections::HashMap, fs, path::Path};

async fn account(root: &Path, user: &str, key: &str) -> DbCache {
    let storage = root.join("db_storage");
    let cache = root.join("cache");
    fs::create_dir_all(storage.join("contact")).unwrap();
    let plain = root.join("fixture.db");
    let conn = encrypted_sqlite::sqlite(&plain);
    conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,extra_buffer BLOB);
        CREATE TABLE contact_label(label_id_,label_name_,sort_order_);
        INSERT INTO contact_label VALUES(7,'Friends',1);").unwrap();
    conn.execute("INSERT INTO contact VALUES(?1,?1,'',0,x'f2010137')", [user])
        .unwrap();
    drop(conn);
    encrypted_sqlite::encrypt(&plain, &storage.join("contact/contact.db"));
    DbCache::with_dirs(
        storage,
        cache.clone(),
        cache.join("_mtimes.json"),
        HashMap::from([(key.to_owned(), "11".repeat(32))]),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn formal_contacts_and_tags_remain_bound_to_the_selected_account() {
    let root = tempfile::tempdir().unwrap();
    let [primary, compatibility] = crate::adapters::wechat::contacts::source_keys();
    assert_eq!(primary, "contact/contact.db");
    assert_eq!(compatibility, "contact\\contact.db");
    let first = account(&root.path().join("first"), "alice", primary).await;
    let second = account(&root.path().join("second"), "bob", primary).await;
    for (db, user) in [(&first, "alice"), (&second, "bob"), (&first, "alice")] {
        let path = db.db_dir().join("contact/contact.db");
        let before = fs::read(&path).unwrap();
        let names = load_names(db).await.unwrap();
        let contacts = q_contacts(&names, None, 10).await.unwrap();
        assert_eq!(contacts["total"], 1);
        assert_eq!(contacts["contacts"][0]["username"], user);
        assert_eq!(contacts["contacts"][0].as_object().unwrap().len(), 2);
        let names = HashMap::from([(user.to_owned(), "Display override".to_owned())]);
        let tags = mcp_contacts::q_contact_tags(db, &names).await.unwrap();
        assert_eq!(tags.total_tags, 1);
        assert_eq!(tags.tags[0].members[0].username, user);
        assert_eq!(tags.tags[0].members[0].display_name, "Display override");
        let tag = mcp_contacts::q_tag_members(db, &names, "Friends")
            .await
            .unwrap();
        assert_eq!(tag.members[0].username, user);
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[tokio::test]
async fn tag_source_spelling_remains_account_scoped() {
    let root = tempfile::tempdir().unwrap();
    let compatibility = crate::adapters::wechat::contacts::source_keys()[1];
    let db = account(root.path(), "alice", compatibility).await;
    let names = HashMap::from([("alice".into(), "Display override".into())]);
    let tags = mcp_contacts::q_contact_tags(&db, &names).await.unwrap();
    assert_eq!(tags.tags[0].members[0].username, "alice");
    assert_eq!(tags.tags[0].members[0].display_name, "Display override");
    let members = mcp_contacts::q_tag_members(&db, &names, "Friends")
        .await
        .unwrap();
    assert_eq!(members.members[0].username, "alice");
}

#[tokio::test]
async fn primary_contact_source_error_does_not_fall_back_to_a_valid_alias() {
    let root = tempfile::tempdir().unwrap();
    let [primary, compatibility] = crate::adapters::wechat::contacts::source_keys();
    let original = account(&root.path().join("account"), "alice", compatibility).await;
    let cache = root.path().join("conflict-cache");
    let db = DbCache::with_dirs(
        original.db_dir().to_owned(),
        cache.clone(),
        cache.join("_mtimes.json"),
        HashMap::from([
            (primary.to_owned(), "22".repeat(32)),
            (compatibility.to_owned(), "11".repeat(32)),
        ]),
    )
    .await
    .unwrap();
    assert!(load_names(&db).await.is_err());
    assert!(mcp_contacts::q_contact_tags(&db, &HashMap::new())
        .await
        .is_err());
    assert!(mcp_contacts::q_tag_members(&db, &HashMap::new(), "Friends")
        .await
        .is_err());
}
