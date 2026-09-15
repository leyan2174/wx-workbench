use super::*;
use crate::adapters::wechat::messages::SourceFile;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;

const USERNAME: &str = "synthetic-page-peer";

#[test]
fn continuation_distinguishes_empty_short_and_full_pages_without_extra_reads() {
    for count in 0..=3 {
        let root = tempfile::tempdir().unwrap();
        let rows = (0..count)
            .map(|index| (index + 1, 100 + index, "needle"))
            .collect::<Vec<_>>();
        let snapshot =
            Snapshot::open(vec![database(root.path(), 0, &rows)], [USERNAME.to_owned()]).unwrap();
        let page = domain::Page {
            limit: 2,
            offset: 0,
            oldest_first: false,
        };
        let history = snapshot
            .history_page(
                USERNAME,
                &domain::Filter::default(),
                &LegacyReadPolicy::default(),
                &page,
            )
            .unwrap();
        let search = snapshot
            .search_page(
                None,
                &domain::Filter::default(),
                &LegacyReadPolicy::default(),
                "needle",
                &page,
            )
            .unwrap();
        let incremental = snapshot
            .new_messages_page(&[(USERNAME.to_owned(), 99)], &page)
            .unwrap();
        let expected = if count < 2 {
            domain::PageContinuation::Exhausted
        } else {
            domain::PageContinuation::MayHaveMore
        };
        for read in [history, search, incremental] {
            assert_eq!(read.page.completeness, domain::Completeness::Complete);
            assert_eq!(read.page.continuation, expected);
            assert_eq!(read.page.messages.len(), (count as usize).min(2));
        }
        let past_end = snapshot
            .history_page(
                USERNAME,
                &domain::Filter::default(),
                &LegacyReadPolicy::default(),
                &domain::Page {
                    limit: 2,
                    offset: 10,
                    oldest_first: false,
                },
            )
            .unwrap();
        assert!(past_end.page.messages.is_empty());
        assert_eq!(
            past_end.page.continuation,
            domain::PageContinuation::Exhausted
        );
    }
}

#[test]
fn continuation_preserves_cross_shard_duplicates_and_is_conservative_on_full_page() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "identical")]),
            database(root.path(), 1, &[(7, 100, "identical")]),
        ],
        [USERNAME.to_owned()],
    )
    .unwrap();
    for (limit, continuation) in [
        (2, domain::PageContinuation::MayHaveMore),
        (3, domain::PageContinuation::Exhausted),
    ] {
        let read = snapshot
            .history_page(
                USERNAME,
                &domain::Filter::default(),
                &LegacyReadPolicy::default(),
                &domain::Page {
                    limit,
                    offset: 0,
                    oldest_first: false,
                },
            )
            .unwrap();
        assert_eq!(read.page.messages.len(), 2);
        assert_ne!(
            read.page.messages[0].reference,
            read.page.messages[1].reference
        );
        assert_eq!(read.page.continuation, continuation);
    }
}

#[test]
fn deduplication_does_not_prove_exhaustion_of_a_bounded_candidate_read() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "seen")]),
            database(root.path(), 1, &[(8, 200, "not read yet")]),
        ],
        [USERNAME.to_owned()],
    )
    .unwrap();
    let raw = snapshot
        .read_page(0, &domain::Filter::default(), 1, true)
        .unwrap()
        .remove(0);
    let candidates = vec![
        snapshot.decoded_candidate(&raw).unwrap(),
        snapshot.decoded_candidate(&raw).unwrap(),
    ];
    let read = selected(
        candidates,
        &domain::Page {
            limit: 2,
            offset: 0,
            oldest_first: true,
        },
        false,
        PageDiagnostics::default(),
        false,
    )
    .unwrap();
    assert_eq!(read.page.messages.len(), 1);
    assert_eq!(read.page.completeness, domain::Completeness::Complete);
    assert_eq!(
        read.page.continuation,
        domain::PageContinuation::MayHaveMore
    );
}

fn database(root: &Path, index: usize, rows: &[(i64, i64, &str)]) -> SourceFile {
    let path = root.join(format!("message_{index}.db"));
    let conn = Connection::open(&path).unwrap();
    let table = crate::adapters::wechat::messages::read::layout::table_for_username(USERNAME);
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,
        create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)"))
        .unwrap();
    for (id, timestamp, text) in rows {
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(?1,1,?2,NULL,?3,0)"),
            params![id, timestamp, text],
        )
        .unwrap();
    }
    SourceFile {
        logical_name: format!("message/message_{index}.db"),
        path,
        kind: SourceKind::Ordinary,
    }
}

fn wire(read: &ReadPage) -> Vec<Value> {
    read.page
        .messages
        .iter()
        .map(|message| {
            let mut value =
                serde_json::to_value(read.legacy.message(&message.reference).unwrap()).unwrap();
            value["timestamp"] = json!(message.timestamp);
            value["text"] = json!(message.preview);
            value
        })
        .collect()
}

#[test]
fn typed_pages_preserve_global_history_order_endpoints_and_legacy_wire() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "a100"), (8, 300, "a300")]),
            database(root.path(), 1, &[(7, 100, "b100"), (9, 200, "b200")]),
        ],
        [USERNAME.to_owned()],
    )
    .unwrap();
    let read = snapshot
        .history_page(
            USERNAME,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            &domain::Page {
                limit: 3,
                offset: 1,
                oldest_first: false,
            },
        )
        .unwrap();
    assert_eq!(read.page.completeness, domain::Completeness::Complete);
    assert_eq!(read.page.unresolved_conversations(), 0);
    assert_eq!(
        wire(&read),
        vec![
            json!({"local_id":7,"source":"message/message_0.db","type":"文本","timestamp":100,"text":"a100"}),
            json!({"local_id":7,"source":"message/message_1.db","type":"文本","timestamp":100,"text":"b100"}),
            json!({"local_id":9,"source":"message/message_1.db","type":"文本","timestamp":200,"text":"b200"}),
        ]
    );
    assert_ne!(
        read.page.messages[0].reference,
        read.page.messages[1].reference
    );
    assert_eq!(read.diagnostics.hits, 2);
    assert_eq!(read.diagnostics.shards[0].latest_timestamp, 300);
    let bounded = snapshot
        .history_page(
            USERNAME,
            &domain::Filter {
                since: Some(100),
                until: Some(200),
                kinds: vec![],
            },
            &LegacyReadPolicy::default(),
            &domain::Page {
                limit: 10,
                offset: 0,
                oldest_first: true,
            },
        )
        .unwrap();
    assert_eq!(
        bounded
            .page
            .messages
            .iter()
            .map(|m| m.timestamp)
            .collect::<Vec<_>>(),
        [100, 100, 200]
    );
}

#[test]
fn history_same_second_rank_uses_latest_shard_not_filename_order() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "a"), (8, 200, "a-new")]),
            database(root.path(), 1, &[(7, 100, "b"), (9, 300, "b-new")]),
        ],
        [USERNAME.to_owned()],
    )
    .unwrap();
    let read = snapshot
        .history_page(
            USERNAME,
            &domain::Filter {
                since: Some(100),
                until: Some(100),
                kinds: vec![],
            },
            &LegacyReadPolicy::default(),
            &domain::Page {
                limit: 1,
                offset: 0,
                oldest_first: true,
            },
        )
        .unwrap();
    assert_eq!(read.page.messages[0].preview, "b");
    assert_eq!(
        read.diagnostics.shards[0].logical_source,
        "message/message_1.db"
    );
}

#[test]
fn search_keeps_unknown_conversations_without_session_inventory_and_reverses_ties() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "needle"), (8, 300, "needle")]),
            database(root.path(), 1, &[(7, 100, "needle"), (9, 200, "needle")]),
        ],
        [],
    )
    .unwrap();
    let read = snapshot
        .search_page(
            None,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            "needle",
            &domain::Page {
                limit: 4,
                offset: 0,
                oldest_first: false,
            },
        )
        .unwrap();
    assert_eq!(read.page.unresolved_conversations(), 4);
    assert!(read
        .page
        .messages
        .iter()
        .all(|m| matches!(m.conversation, Conversation::Unmapped(_))));
    assert_eq!(
        read.page
            .messages
            .iter()
            .map(|m| m.timestamp)
            .collect::<Vec<_>>(),
        [300, 200, 100, 100]
    );
    let projected = wire(&read);
    assert_eq!(projected[2]["source"], "message/message_1.db");
    assert_eq!(projected[3]["source"], "message/message_0.db");
    let table = crate::adapters::wechat::messages::read::layout::table_for_username(USERNAME);
    assert_eq!(projected[0]["unmapped_conversation"], &table[4..]);
    assert_eq!(
        read.legacy
            .message(&read.page.messages[0].reference)
            .unwrap()
            .unmapped_chat_label(),
        Some(table.as_str())
    );
    let targets = HashSet::from([USERNAME.to_owned()]);
    assert!(snapshot
        .search_page(
            Some(&targets),
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            "needle",
            &domain::Page {
                limit: 4,
                offset: 0,
                oldest_first: false
            }
        )
        .unwrap()
        .page
        .messages
        .is_empty());
}

#[test]
fn bad_candidate_in_an_unselected_shard_is_still_a_failure() {
    let root = tempfile::tempdir().unwrap();
    let valid = database(root.path(), 0, &[(7, 100, "needle")]);
    let bad = database(root.path(), 1, &[]);
    let table = crate::adapters::wechat::messages::read::layout::table_for_username(USERNAME);
    Connection::open(&bad.path)
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO [{table}] VALUES('invalid-id',1,1,NULL,'needle',0)"
        ))
        .unwrap();
    let snapshot = Snapshot::open(vec![valid, bad], [USERNAME.to_owned()]).unwrap();
    let page = domain::Page {
        limit: 1,
        offset: 0,
        oldest_first: false,
    };
    assert!(snapshot
        .history_page(
            USERNAME,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            &page
        )
        .is_err());
    assert!(snapshot
        .search_page(
            None,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            "needle",
            &page
        )
        .is_err());
}

#[test]
fn legacy_projection_is_bound_to_selected_opaque_references_and_live_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let file = database(root.path(), 0, &[(7, 100, "same")]);
    let first = Snapshot::open(vec![file.clone()], [USERNAME.to_owned()]).unwrap();
    let second = Snapshot::open(vec![file], [USERNAME.to_owned()]).unwrap();
    let page = domain::Page {
        limit: 1,
        offset: 0,
        oldest_first: false,
    };
    let read = first
        .history_page(
            USERNAME,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            &page,
        )
        .unwrap();
    let other = second
        .history_page(
            USERNAME,
            &domain::Filter::default(),
            &LegacyReadPolicy::default(),
            &page,
        )
        .unwrap();
    assert!(read
        .legacy
        .message(&other.page.messages[0].reference)
        .is_err());
    drop(first);
    assert_eq!(
        read.legacy
            .message(&read.page.messages[0].reference)
            .unwrap_err()
            .downcast_ref::<domain::Error>(),
        Some(&domain::Error::Expired)
    );
}

#[test]
fn subscription_page_preserves_exclusive_start_and_global_oldest_limit() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::open(
        vec![
            database(root.path(), 0, &[(7, 100, "old"), (8, 102, "later")]),
            database(root.path(), 1, &[(9, 101, "first")]),
        ],
        [USERNAME.to_owned()],
    )
    .unwrap();
    let page = domain::Page {
        limit: 1,
        offset: 0,
        oldest_first: true,
    };
    let read = snapshot
        .new_messages_page(&[(USERNAME.into(), 100)], &page)
        .unwrap();
    assert_eq!(read.page.messages.len(), 1);
    assert_eq!(read.page.messages[0].timestamp, 101);
    assert_eq!(read.page.messages[0].preview, "first");
    assert!(snapshot
        .new_messages_page(&[(USERNAME.into(), i64::MAX)], &page)
        .is_err());
}
