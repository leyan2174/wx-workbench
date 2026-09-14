//! Existing synthetic query regressions now exercise the production snapshot adapter.
use super::*;
use crate::{
    adapters::wechat::messages::{LegacyReadPolicy, Snapshot, SourceFile},
    business::messages as domain,
};

#[test]
fn history_validation_preserves_legacy_order_and_diagnostics() {
    let types = vec![49; 101];
    let mut options = HistoryQuery {
        page: MessagePage {
            limit: 0,
            offset: usize::MAX,
        },
        filter: MessageFilter {
            since: Some(2),
            until: Some(1),
            msg_type: Some(49),
        },
        meta: MetaOptions::default(),
        msg_types: Some(&types),
        oldest_first: false,
    };
    let check = |options: &HistoryQuery<'_>, expected: &str| {
        let error = message_read::validate_history(options).unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
        assert!(error.downcast_ref::<domain::Error>().is_some());
    };
    check(&options, "conflicting history");
    options.filter.msg_type = None;
    check(&options, "too many history types");
    options.msg_types = None;
    check(&options, "history limit");
    options.page.limit = 1;
    check(&options, "time range");
    options.filter.until = Some(2);
    check(&options, "history page overflow");
    options.page.offset = i64::MAX as usize;
    check(&options, "SQLite integer range");
    options.page.offset = 100_000;
    assert_eq!(message_read::validate_history(&options).unwrap(), 100_001);
}

fn read(
    conn: &Connection,
    table: &str,
    view: MessageView<'_>,
    filter: MessageFilter,
    page: MessagePage,
    keyword: Option<&str>,
) -> Result<Vec<Value>> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("snapshot.db");
    conn.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    let target = format!("Msg_{:x}", md5::compute(view.username.as_bytes()));
    {
        let copy = Connection::open(&path)?;
        // Historical fixtures used Msg_test; the adapter tests real production table identities.
        anyhow::ensure!(
            table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid test table"
        );
        if table != target {
            copy.execute_batch(&format!("ALTER TABLE [{table}] RENAME TO [{target}]"))?;
        }
    }
    let mut identities: Vec<_> = view.names.keys().cloned().collect();
    identities.push(view.username.to_owned());
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path,
            kind: domain::SourceKind::Ordinary,
        }],
        identities,
    )?;
    let names = Names {
        map: view.names.clone(),
        md5_to_uname: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    };
    let policy = LegacyReadPolicy {
        local_types: filter.msg_type.into_iter().collect(),
    };
    let filter = domain::Filter {
        since: filter.since,
        until: filter.until,
        kinds: Vec::new(),
    };
    read_selected(&snapshot, &names, view, filter, policy, page, keyword)
}

fn read_selected(
    snapshot: &Snapshot,
    names: &Names,
    view: MessageView<'_>,
    filter: domain::Filter,
    policy: LegacyReadPolicy,
    page: MessagePage,
    keyword: Option<&str>,
) -> Result<Vec<Value>> {
    let page = domain::Page {
        limit: page.limit,
        offset: page.offset,
        oldest_first: false,
    };
    let mut result = Vec::new();
    for stream in snapshot.streams_for(view.username, domain::SourceKind::Ordinary) {
        let rows = if let Some(keyword) = keyword {
            snapshot.search_legacy_page(
                stream,
                &filter,
                &policy,
                keyword,
                page.candidate_limit()?,
            )?
        } else {
            snapshot.read_legacy_page(stream, &filter, &policy, page.candidate_limit()?, false)?
        };
        for raw in rows {
            result.push(domain::Candidate {
                order: snapshot.order_key(&raw)?,
                reference: raw.reference.clone(),
                value: message_read::project(snapshot, &raw, names, view.group_nicknames)?,
            });
        }
    }
    Ok(page.select(result, domain::Completeness::Complete)?)
}

pub(super) fn query_messages(
    path: &std::path::Path,
    table: &str,
    view: MessageView<'_>,
    filter: MessageFilter,
    page: MessagePage,
) -> Result<Vec<Value>> {
    read(&Connection::open(path)?, table, view, filter, page, None)
}
pub(super) fn search_in_table(
    conn: &Connection,
    table: &str,
    view: MessageView<'_>,
    keyword: &str,
    filter: MessageFilter,
    limit: usize,
) -> Result<Vec<Value>> {
    read(
        conn,
        table,
        view,
        filter,
        MessagePage { limit, offset: 0 },
        Some(keyword),
    )
}

#[test]
fn unmapped_conversation_is_not_projected_as_an_empty_known_username() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("unmapped.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE Msg_00000000000000000000000000000000(local_id INTEGER,local_type INTEGER,create_time INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO Msg_00000000000000000000000000000000 VALUES(1,1,100,'synthetic',0)").unwrap();
    drop(conn);
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message\\message_0.db".into(),
            path,
            kind: domain::SourceKind::Ordinary,
        }],
        [],
    )
    .unwrap();
    let names = Names {
        map: HashMap::new(),
        md5_to_uname: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    };
    let raw = snapshot
        .read_page(0, &domain::Filter::default(), 1, true)
        .unwrap()
        .remove(0);
    let value = message_read::project(&snapshot, &raw, &names, &HashMap::new()).unwrap();
    assert_eq!(value["source"], "message/message_0.db");
    assert_eq!(raw.logical_source, "message\\message_0.db");
    assert!(value["username"].is_null());
    assert_eq!(value["identity_status"], "unmapped");
    assert_eq!(
        value["unmapped_conversation"],
        "00000000000000000000000000000000"
    );
    assert!(value.get("raw_content").is_none());
}

#[test]
fn malformed_structured_content_is_visible_without_changing_valid_text() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("content.db");
    let conn = Connection::open(&path).unwrap();
    let table = format!("Msg_{:x}", md5::compute("wxid_content"));
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}] VALUES(1,49,100,'<msg>',0),(2,1,100,'literal text',0)")).unwrap();
    drop(conn);
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path,
            kind: domain::SourceKind::Ordinary,
        }],
        ["wxid_content".into()],
    )
    .unwrap();
    let names = Names {
        map: HashMap::new(),
        md5_to_uname: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    };
    let rows = snapshot
        .read_page(0, &domain::Filter::default(), 10, true)
        .unwrap();
    let bad = message_read::project(&snapshot, &rows[0], &names, &HashMap::new()).unwrap();
    assert_eq!(bad["content_issue"], "malformed_content");
    assert!(bad.get("rich").is_none());
    assert_ne!(bad["content"], "<msg>");
    let valid = message_read::project(&snapshot, &rows[1], &names, &HashMap::new()).unwrap();
    assert_eq!(valid["content"], "literal text");
    assert!(valid.get("content_issue").is_none());
}
