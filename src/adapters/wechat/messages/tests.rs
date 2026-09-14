use super::read::logical_name;
use super::*;
use crate::business::messages::{self as domain, Conversation, MessageSelector, SourceKind};
use rusqlite::Connection;

fn database(root: &std::path::Path, index: usize, rows: usize) -> SourceFile {
    let path = root.join(format!("{index}.db"));
    let conn = Connection::open(&path).unwrap();
    let table = format!("Msg_{:x}", md5::compute("wxid_test"));
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content,WCDB_CT_message_content INTEGER)")).unwrap();
    for _ in 0..rows {
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(7,1,100,1,'same',0)"),
            [],
        )
        .unwrap();
    }
    SourceFile {
        logical_name: format!("message/message_{index}.db"),
        path,
        kind: SourceKind::Ordinary,
    }
}

#[test]
fn empty_target_still_requires_requested_body_projection() {
    for column in ["message_content", "WCDB_CT_message_content"] {
        let root = tempfile::tempdir().unwrap();
        let populated = database(root.path(), 0, 1);
        let empty = database(root.path(), 1, 0);
        let table = format!("Msg_{:x}", md5::compute("wxid_test"));
        let conn = Connection::open(&empty.path).unwrap();
        conn.execute_batch(&format!("ALTER TABLE [{table}] DROP COLUMN [{column}]"))
            .unwrap();
        drop(conn);
        let snapshot = Snapshot::open(vec![populated, empty], ["wxid_test".into()]).unwrap();
        let reference = snapshot
            .resolve(
                &MessageSelector {
                    username: "wxid_test",
                    local_id: 7,
                    timestamp: None,
                },
                SourceKind::Ordinary,
            )
            .unwrap();
        assert_eq!(
            snapshot
                .read_metadata(reference.evidence())
                .unwrap()
                .local_id,
            Some(7)
        );
        let error = snapshot
            .require_target_content("wxid_test", SourceKind::Ordinary)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<domain::Error>(),
            Some(&domain::Error::Unsupported)
        );
        let error = snapshot
            .read_page(1, &domain::Filter::default(), 10, true)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<domain::Error>(),
            Some(&domain::Error::Unsupported)
        );
        snapshot
            .require_target_content("wxid_absent", SourceKind::Ordinary)
            .unwrap();
    }
}

#[test]
fn candidate_limit_preserves_same_second_order_in_both_directions() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(vec![database(root.path(), 0, 3)], ["wxid_test".into()]).unwrap();
    let ascending = snapshot
        .read_page(0, &domain::Filter::default(), 2, true)
        .unwrap();
    let descending = snapshot
        .read_page(0, &domain::Filter::default(), 2, false)
        .unwrap();
    let keys = |rows: Vec<RawMessage>| {
        rows.iter()
            .map(|row| snapshot.order_key(row).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(ascending), keys(descending));
}

#[test]
fn large_requested_page_is_not_an_actual_candidate_budget_overflow() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(vec![database(root.path(), 0, 1)], ["wxid_test".into()]).unwrap();
    for limit in [100_001, i64::MAX as usize] {
        let rows = snapshot
            .read_page(0, &domain::Filter::default(), limit, true)
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
    let error = snapshot
        .read_page(0, &domain::Filter::default(), usize::MAX, true)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<domain::Error>(),
        Some(&domain::Error::Limit)
    );
}

#[test]
fn large_offset_over_small_inventory_returns_an_empty_page() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(vec![database(root.path(), 0, 1)], ["wxid_test".into()]).unwrap();
    let page = domain::Page {
        limit: 1,
        offset: 100_000,
        oldest_first: true,
    };
    let rows = snapshot
        .read_page(
            0,
            &domain::Filter::default(),
            page.candidate_limit().unwrap(),
            true,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let candidates = rows
        .into_iter()
        .map(|row| domain::Candidate {
            order: snapshot.order_key(&row).unwrap(),
            reference: row.reference.clone(),
            value: row,
        })
        .collect();
    assert!(page
        .select(candidates, domain::Completeness::Complete)
        .unwrap()
        .is_empty());
}

#[test]
fn sqlite_case_variants_preserve_raw_source_and_do_not_write() {
    let root = tempfile::tempdir().unwrap();
    let mut file = database(root.path(), 0, 1);
    file.logical_name = "MESSAGE\\MESSAGE_0.DB".into();
    let conn = Connection::open(&file.path).unwrap();
    let table = format!("Msg_{:x}", md5::compute("wxid_test"));
    conn.execute_batch(&format!(
        "ALTER TABLE [{table}] RENAME TO TemporaryName; ALTER TABLE TemporaryName RENAME TO [{}]",
        table.to_ascii_uppercase()
    ))
    .unwrap();
    drop(conn);
    let before = std::fs::read(&file.path).unwrap();
    let snapshot = Snapshot::open(vec![file.clone()], ["wxid_test".into()]).unwrap();
    let reference = snapshot
        .resolve(
            &MessageSelector {
                username: "wxid_test",
                local_id: 7,
                timestamp: Some(100),
            },
            SourceKind::Ordinary,
        )
        .unwrap();
    let raw = snapshot.read_evidence(reference.evidence()).unwrap();
    assert_eq!(raw.logical_source, "MESSAGE\\MESSAGE_0.DB");
    assert_eq!(
        snapshot.conversation(&reference).unwrap(),
        &Conversation::Known("wxid_test".into())
    );
    drop(snapshot);
    assert_eq!(before, std::fs::read(&file.path).unwrap());
}

#[test]
fn every_shadowed_rowid_alias_is_rejected_in_message_and_sender_tables() {
    for sender in [false, true] {
        for alias in ["rowid", "_rowid_", "oid"] {
            for generated in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let file = database(root.path(), 0, 1);
                let conn = Connection::open(&file.path).unwrap();
                conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT)")
                    .unwrap();
                let table = if sender {
                    "Name2Id".into()
                } else {
                    format!("Msg_{:x}", md5::compute("wxid_test"))
                };
                let definition = if generated {
                    "INTEGER GENERATED ALWAYS AS (1) VIRTUAL"
                } else {
                    "INTEGER"
                };
                conn.execute_batch(&format!(
                    "ALTER TABLE [{table}] ADD COLUMN [{alias}] {definition}"
                ))
                .unwrap();
                drop(conn);
                let error = Snapshot::open(vec![file], ["wxid_test".into()])
                    .err()
                    .expect("shadowed row identity must fail");
                assert_eq!(
                    error.downcast_ref::<super::read::SchemaIssue>(),
                    Some(&super::read::SchemaIssue::ShadowedSqliteRowid)
                );
                assert_eq!(
                    error.downcast_ref::<domain::Error>(),
                    Some(&domain::Error::Unsupported)
                );
            }
        }
    }
}

#[test]
fn duplicate_local_ids_in_one_or_multiple_shards_are_not_identities() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![database(root.path(), 0, 2), database(root.path(), 1, 1)],
        ["wxid_test".into()],
    )
    .unwrap();
    let error = snapshot
        .resolve(
            &MessageSelector {
                username: "wxid_test",
                local_id: 7,
                timestamp: Some(100),
            },
            SourceKind::Ordinary,
        )
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<domain::Error>(),
        Some(&domain::Error::Ambiguous)
    );
    let rows = snapshot
        .read_page(0, &domain::Filter::default(), 10, true)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].reference, rows[1].reference);
    for row in rows {
        snapshot.revalidate(&row.reference).unwrap();
    }
}

#[test]
fn reference_expires_and_cannot_be_used_in_another_account_read() {
    let root = tempfile::tempdir().unwrap();
    let file = database(root.path(), 0, 1);
    let path = file.path.clone();
    let snapshot = Snapshot::open(vec![file], ["wxid_test".into()]).unwrap();
    let reference = snapshot
        .resolve(
            &MessageSelector {
                username: "wxid_test",
                local_id: 7,
                timestamp: None,
            },
            SourceKind::Ordinary,
        )
        .unwrap();
    let another = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path,
            kind: SourceKind::Ordinary,
        }],
        ["wxid_test".into()],
    )
    .unwrap();
    assert_eq!(
        another
            .revalidate(&reference)
            .unwrap_err()
            .downcast_ref::<domain::Error>(),
        Some(&domain::Error::Expired)
    );
    drop(snapshot);
    assert!(reference.evidence().is_expired());
}

#[test]
fn inventory_does_not_require_sessions_or_contact_membership() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(vec![database(root.path(), 0, 1)], []).unwrap();
    assert_eq!(snapshot.streams().len(), 1);
    assert!(matches!(
        snapshot.streams()[0].conversation,
        Conversation::Unmapped(_)
    ));
    let message = snapshot
        .read_page(0, &domain::Filter::default(), 1, false)
        .unwrap()
        .remove(0);
    assert_eq!(snapshot.message(&message).unwrap().kind, domain::Kind::Text);
}

#[test]
fn caller_paths_cannot_be_smuggled_in_logical_source_names() {
    for name in [
        "../message_0.db",
        "C:/message/message_0.db",
        "message/message_0.db-wal",
        "message/message_0_resource.db",
    ] {
        assert!(logical_name(name, SourceKind::Ordinary).is_err());
    }
    assert!(logical_name("message/biz_message_0.db", SourceKind::Ordinary).is_err());
    assert!(logical_name("message/biz_message_0.db", SourceKind::OfficialPush).is_ok());
}

#[test]
fn call_media_is_unknown_even_when_client_status_says_video() {
    let event = call_event("<voipmsg><msg>Video call</msg></voipmsg>");
    assert_eq!(event.media, domain::CallMedia::Unknown);
    assert_eq!(event.status_text.as_deref(), Some("Video call"));
    assert_eq!(call_event("<broken").media, domain::CallMedia::Unknown);
}

#[test]
fn metadata_only_sources_keep_server_scalar_types_and_do_not_fake_body_null() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata.db");
    let conn = Connection::open(&path).unwrap();
    let table = format!("Msg_{:x}", md5::compute("wxid_test"));
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id);
        INSERT INTO [{table}] VALUES(7,34,100,42),(8,34,100,'0009223372036854775808'),(9,34,100,NULL),(10,34,100,0)")).unwrap();
    drop(conn);
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path,
            kind: SourceKind::Ordinary,
        }],
        ["wxid_test".into()],
    )
    .unwrap();
    assert!(!snapshot.streams()[0].supports_content());
    for id in 7..=10 {
        let reference = snapshot
            .resolve(
                &MessageSelector {
                    username: "wxid_test",
                    local_id: id,
                    timestamp: Some(100),
                },
                SourceKind::Ordinary,
            )
            .unwrap();
        let row = snapshot.read_metadata(reference.evidence()).unwrap();
        assert!(matches!(row.content, StoredContent::AbsentColumn));
        if id == 7 {
            assert_eq!(row.checked_server_id().unwrap(), Some(42));
        } else {
            assert!(row.checked_server_id().is_err());
        }
        if id == 8 {
            assert_eq!(
                row.server_id,
                StoredScalar::Text("0009223372036854775808".into())
            );
        }
        assert_eq!(
            snapshot
                .read_evidence(reference.evidence())
                .unwrap_err()
                .downcast_ref::<domain::Error>(),
            Some(&domain::Error::Unsupported)
        );
    }
}

#[test]
fn server_identity_is_unique_before_timestamp_validation() {
    let root = tempfile::tempdir().unwrap();
    let files = vec![database(root.path(), 0, 1), database(root.path(), 1, 1)];
    let table = format!("Msg_{:x}", md5::compute("wxid_test"));
    for (index, file) in files.iter().enumerate() {
        let conn = Connection::open(&file.path).unwrap();
        conn.execute_batch(&format!("ALTER TABLE [{table}] ADD COLUMN server_id INTEGER; UPDATE [{table}] SET server_id=42,create_time={}", 100 + index)).unwrap();
    }
    let snapshot = Snapshot::open(files, ["wxid_test".into()]).unwrap();
    for time in [None, Some(100)] {
        let error = snapshot
            .resolve_server_id("wxid_test", 42, time, SourceKind::Ordinary)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<domain::Error>(),
            Some(&domain::Error::Ambiguous)
        );
    }
}
