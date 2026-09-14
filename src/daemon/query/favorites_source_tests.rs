//! Synthetic encrypted sources through the actual daemon query projection.
use super::{encrypted_cache::encrypted_sqlite, q_favorites, DbCache};
use std::{collections::HashMap, fs};

async fn account(root: &std::path::Path, author: &str) -> DbCache {
    let storage = root.join("db_storage");
    let cache = root.join("cache");
    fs::create_dir_all(storage.join("favorite")).unwrap();
    let plain = root.join("synthetic.db");
    let conn = encrypted_sqlite::sqlite(&plain);
    conn.execute_batch(
        "CREATE TABLE fav_db_item (local_id INTEGER, type INTEGER,
        update_time INTEGER, content TEXT, fromusr TEXT, realchatname TEXT)",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO fav_db_item VALUES(7,5,1700000001000,?1,?2,NULL)",
        rusqlite::params!["<link>https://example.test/article</link>", author],
    )
    .unwrap();
    drop(conn);
    encrypted_sqlite::encrypt(&plain, &storage.join("favorite/favorite.db"));
    DbCache::with_dirs(
        storage,
        cache.clone(),
        cache.join("_mtimes.json"),
        HashMap::from([("favorite/favorite.db".into(), "11".repeat(32))]),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn favorite_query_uses_real_account_caches_and_preserves_public_projection() {
    let root = tempfile::tempdir().unwrap();
    let first = account(&root.path().join("first"), "account_a_author").await;
    let second = account(&root.path().join("second"), "account_b_author").await;
    let first_result = q_favorites(&first, 10, Some(5), None).await.unwrap();
    let second_result = q_favorites(&second, 10, Some(5), None).await.unwrap();
    for (result, author) in [
        (&first_result, "account_a_author"),
        (&second_result, "account_b_author"),
    ] {
        assert_eq!(result["count"], 1);
        assert_eq!(result["items"][0]["id"], 7);
        assert_eq!(result["items"][0]["favorite_id"], "favorite:7");
        assert_eq!(result["has_more"], false);
        assert_eq!(result["items"][0]["type_num"], 5);
        assert_eq!(result["items"][0]["timestamp"], 1700000001i64);
        assert_eq!(result["items"][0]["from"], author);
        assert_eq!(result["items"][0]["chat"], "");
        assert_eq!(result["items"][0]["url"], "https://example.test/article");
    }
    let missing = q_favorites(&first, 10, Some(2), None).await.unwrap();
    assert_eq!(missing["count"], 0);
    assert_eq!(
        q_favorites(&first, 10, Some(5), None).await.unwrap(),
        first_result
    );
}
