use super::{chat_identity::*, strict_message, DbCache, Names};
use crate::business::messages::Error;
use crate::daemon::query::{
    self, encrypted_cache, AttachmentQuery, HistoryQuery, MessageFilter, MessagePage, MetaOptions,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::{collections::HashMap, fs};

const SESSION_IDS: &[&str] = &[
    "legacy.account",
    "external@openim",
    "enterprise@qy_g",
    "wxid_session",
    "synthetic@chatroom",
    "filehelper",
    "brandsessionholder",
    "@placeholder_foldgroup",
];
const TABLE_ONLY: &str = "table_only@openim";

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
}

async fn fixture(populated: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let db_dir = root.path().join("wxid_synthetic_abcd/db_storage");
    let cache_dir = root.path().join("synthetic-cache");
    fs::create_dir_all(&cache_dir).unwrap();
    let mut keys = HashMap::new();
    let mut mtimes = serde_json::Map::new();
    for source in [
        "contact/contact.db",
        "session/session.db",
        "message/message_0.db",
    ] {
        fs::create_dir_all(db_dir.join(source).parent().unwrap()).unwrap();
        let path = cache_dir.join(format!("{:x}.db", md5::compute(source)));
        let conn = encrypted_cache::sqlite(&path);
        match source {
            "contact/contact.db" => {
                conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,local_type INTEGER,alias TEXT,description TEXT,extra_buffer BLOB);").unwrap();
                if populated {
                    for (username, display) in [
                        ("contact_only", "Contact Only"),
                        ("wxid_duplicate_a", "Same Name"),
                        ("wxid_duplicate_b", "Same Name"),
                        ("display_collision", "legacy.account"),
                        ("wxid_partial_a", "Partial A"),
                        ("wxid_partial_b", "Partial B"),
                    ] {
                        conn.execute(
                            "INSERT INTO contact VALUES(?1,?2,'',0,0,'','',NULL)",
                            params![username, display],
                        )
                        .unwrap();
                    }
                } else {
                    // A usable isolated account needs its own nonempty contact directory.
                    conn.execute(
                        "INSERT INTO contact VALUES(?1,?2,'',0,0,'','',NULL)",
                        params!["isolated_contact", "Isolated Contact"],
                    )
                    .unwrap();
                }
            }
            "session/session.db" => {
                conn.execute_batch("CREATE TABLE SessionTable(username TEXT,unread_count INTEGER,summary TEXT,last_timestamp INTEGER,last_msg_type INTEGER,last_msg_sender TEXT,last_sender_display_name TEXT);").unwrap();
                if populated {
                    for username in SESSION_IDS {
                        conn.execute(
                            "INSERT INTO SessionTable VALUES(?1,0,'synthetic',100,1,'','')",
                            [username],
                        )
                        .unwrap();
                    }
                }
            }
            _ => {
                conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT)")
                    .unwrap();
                if populated {
                    // Most session identities deliberately have no Name2Id row.
                    conn.execute(
                        "INSERT INTO Name2Id VALUES(?1),(?2)",
                        params![TABLE_ONLY, "sender_without_table"],
                    )
                    .unwrap();
                    for username in SESSION_IDS.iter().copied().chain([TABLE_ONLY]) {
                        if username == "brandsessionholder" {
                            continue;
                        }
                        let table = format!("Msg_{:x}", md5::compute(username));
                        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,server_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}] VALUES(1,1,100,0,42,'synthetic',0)")).unwrap();
                    }
                }
            }
        }
        drop(conn);
        let mt = encrypted_cache::seed(&path, &db_dir.join(source));
        mtimes.insert(source.into(), json!({"db_mt":mt,"wal_mt":0,"path":path}));
        keys.insert(source.into(), "11".repeat(32));
    }
    let mtime = cache_dir.join("_mtimes.json");
    fs::write(&mtime, serde_json::to_vec(&mtimes).unwrap()).unwrap();
    let db = DbCache::with_dirs(db_dir, cache_dir, mtime, keys)
        .await
        .unwrap();
    let mut names = query::load_names(&db).await.unwrap();
    names.msg_db_keys.push("message/message_0.db".into());
    Fixture {
        _root: root,
        db,
        names,
    }
}

fn history_options() -> HistoryQuery<'static> {
    HistoryQuery {
        page: MessagePage {
            limit: 10,
            offset: 0,
        },
        filter: MessageFilter {
            since: None,
            until: None,
            msg_type: None,
        },
        meta: MetaOptions::default(),
        msg_types: None,
        oldest_first: false,
    }
}

#[tokio::test]
async fn stable_usernames_round_trip_through_history_attachments_and_strict_lookup() {
    let f = fixture(true).await;
    let sessions = query::q_sessions(&f.db, &f.names, 100, false, false)
        .await
        .unwrap();
    let export_list = query::q_export_chat_list(&f.db, &f.names).await.unwrap();
    for row in sessions["sessions"].as_array().unwrap() {
        let username = row["username"].as_str().unwrap();
        assert_eq!(resolve(&f.db, &f.names, username).await.unwrap(), username);
        let exported = export_list["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["username"] == username)
            .unwrap();
        assert_eq!(exported["exportable"], row["exportable"]);
        if username == "brandsessionholder" {
            assert_eq!(row["exportable"], false);
            assert_eq!(
                row["skip_reason"],
                "system_placeholder_without_message_table"
            );
            continue;
        }
        assert_eq!(row["exportable"], true);
        assert!(row.get("skip_reason").is_none());
        let history = query::q_history(&f.db, &f.names, username, history_options())
            .await
            .unwrap();
        assert_eq!(history["username"], username);
        assert_eq!(history["count"], 1);
        let attachments = query::q_attachments(
            &f.db,
            &f.names,
            username,
            AttachmentQuery {
                kinds: Some(vec!["image".into()]),
                page: MessagePage {
                    limit: 10,
                    offset: 0,
                },
                since: None,
                until: None,
                meta: MetaOptions::default(),
            },
        )
        .await
        .unwrap();
        assert_eq!(attachments["username"], username);
        let resolved =
            strict_message::with_resolved(&f.db, &f.names, username, 1, 100, |_, _| Ok(()))
                .await
                .unwrap();
        assert!(matches!(resolved, strict_message::Resolution::Found(())));
    }
}

#[tokio::test]
async fn message_catalog_identity_is_exact_and_round_trips_without_contact_or_session() {
    let f = fixture(true).await;
    let directory = query::q_export_directory_catalog(&f.db, &f.names)
        .await
        .unwrap();
    assert!(directory["chats"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["username"] == TABLE_ONLY));
    assert_eq!(
        resolve(&f.db, &f.names, TABLE_ONLY).await.unwrap(),
        TABLE_ONLY
    );
    let history = query::q_history(&f.db, &f.names, TABLE_ONLY, history_options())
        .await
        .unwrap();
    assert_eq!(history["username"], TABLE_ONLY);
    assert_eq!(history["count"], 1);
    for invalid in [
        "sender_without_table",
        "unknown_deadbeef",
        "wxid_unproven",
        "missing@chatroom",
        "missing@openim",
        "missing@qy_g",
        "external@OPENIM",
        " external@openim",
        "external@openim ",
        "",
        "   ",
        "external@openim' OR 1=1 --",
    ] {
        let error = resolve(&f.db, &f.names, invalid).await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<Error>(), Some(Error::NotFound)),
            "{invalid}: {error:#}"
        );
    }
}

#[tokio::test]
async fn display_names_remain_ambiguous_and_directory_identity_wins() {
    let f = fixture(true).await;
    assert_eq!(
        resolve(&f.db, &f.names, "legacy.account").await.unwrap(),
        "legacy.account"
    );
    assert_eq!(
        resolve(&f.db, &f.names, "contact_only").await.unwrap(),
        "contact_only"
    );
    assert_eq!(
        resolve(&f.db, &f.names, "contact only").await.unwrap(),
        "contact_only"
    );
    assert_eq!(
        resolve(&f.db, &f.names, "Contact On").await.unwrap(),
        "contact_only"
    );
    for display in ["Same Name", "same name", "Partial"] {
        let error = resolve(&f.db, &f.names, display).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<Error>(),
            Some(Error::Ambiguous)
        ));
        assert!(query::q_export_chat(&f.db, &f.names, display)
            .await
            .is_err());
    }
    assert!(require_exact(&f.db, &f.names, "Contact Only")
        .await
        .is_err());
}

#[tokio::test]
async fn identities_do_not_leak_between_accounts() {
    let populated = fixture(true).await;
    let isolated = fixture(false).await;
    assert_eq!(
        resolve(&isolated.db, &isolated.names, "isolated_contact")
            .await
            .unwrap(),
        "isolated_contact"
    );
    assert!(matches!(
        resolve(&populated.db, &populated.names, "isolated_contact")
            .await
            .unwrap_err()
            .downcast_ref::<Error>(),
        Some(Error::NotFound)
    ));
    for username in SESSION_IDS
        .iter()
        .copied()
        .chain([TABLE_ONLY, "contact_only"])
    {
        assert_eq!(
            resolve(&populated.db, &populated.names, username)
                .await
                .unwrap(),
            username
        );
        assert!(matches!(
            resolve(&isolated.db, &isolated.names, username)
                .await
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::NotFound)
        ));
    }
}

#[test]
fn system_labels_do_not_blacklist_real_message_tables_or_other_system_accounts() {
    for username in [
        "brandsessionholder",
        "@placeholder_foldgroup",
        "filehelper",
        "newsapp",
    ] {
        for has_table in [false, true] {
            let mut value: Value = json!({"username":username});
            mark_exportability(&mut value, Some(has_table));
            assert_eq!(value["exportable"], !is_folded(username) || has_table);
        }
    }
}

#[tokio::test]
async fn session_lists_survive_unavailable_message_sources_with_unknown_fold_capability() {
    let mut f = fixture(true).await;
    f.names.msg_db_keys.push("message/message_404.db".into());
    let sessions = query::q_sessions(&f.db, &f.names, 100, false, false)
        .await
        .unwrap();
    let export_list = query::q_export_chat_list(&f.db, &f.names).await.unwrap();
    for rows in [&sessions["sessions"], &export_list["chats"]] {
        assert_eq!(rows.as_array().unwrap().len(), SESSION_IDS.len());
        for row in rows.as_array().unwrap() {
            let username = row["username"].as_str().unwrap();
            if is_folded(username) {
                assert!(row.get("exportable").unwrap().is_null());
                assert_eq!(row["skip_reason"], "message_source_unavailable");
            } else {
                assert_eq!(row["exportable"], true);
                assert!(row.get("skip_reason").is_none());
            }
        }
    }
}

#[tokio::test]
async fn no_configured_message_identities_allow_contact_display_resolution_only() {
    let mut f = fixture(true).await;
    f.names.msg_db_keys.clear();
    assert_eq!(
        resolve(&f.db, &f.names, "Contact On").await.unwrap(),
        "contact_only"
    );
    assert_eq!(
        query::q_resolve_chat(&f.db, &f.names, "Contact Only")
            .await
            .unwrap(),
        "contact_only"
    );
    assert_eq!(
        resolve(&f.db, &f.names, "legacy.account").await.unwrap(),
        "legacy.account"
    );
    assert!(matches!(
        resolve(&f.db, &f.names, TABLE_ONLY)
            .await
            .unwrap_err()
            .downcast_ref::<Error>(),
        Some(Error::NotFound)
    ));
    // Capability discovery remains strict: physical shards not in the configured
    // inventory prevent claiming that a folded session has no message table.
    let sessions = query::q_sessions(&f.db, &f.names, 100, false, false)
        .await
        .unwrap();
    let folded = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["username"] == "brandsessionholder")
        .unwrap();
    assert!(folded["exportable"].is_null());
    assert_eq!(folded["skip_reason"], "message_source_unavailable");
}

#[tokio::test]
async fn unavailable_configured_catalog_never_redirects_to_a_unique_display_match() {
    let mut f = fixture(true).await;
    f.names.msg_db_keys.push("message/message_404.db".into());
    f.names
        .map
        .insert("different_contact".into(), TABLE_ONLY.into());
    for chat in [TABLE_ONLY, "Contact Only", "Contact On", "not present"] {
        assert!(
            matches!(
                resolve(&f.db, &f.names, chat)
                    .await
                    .unwrap_err()
                    .downcast_ref::<Error>(),
                Some(Error::Unavailable)
            ),
            "{chat}"
        );
    }
    for chat in ["Same Name", "Partial"] {
        assert!(matches!(
            resolve(&f.db, &f.names, chat)
                .await
                .unwrap_err()
                .downcast_ref::<Error>(),
            Some(Error::Ambiguous)
        ));
    }
    assert_eq!(
        resolve(&f.db, &f.names, "contact_only").await.unwrap(),
        "contact_only"
    );
}
