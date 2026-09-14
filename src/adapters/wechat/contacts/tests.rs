use super::*;
use crate::business::contacts::{ContactQuery, ContactView};

fn fixture() -> (tempfile::TempDir, SqliteContacts) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("contacts.db");
    Connection::open(&path).unwrap().execute_batch(
        "CREATE TABLE contact(id INTEGER, username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER, local_type INTEGER, phone_number TEXT);
         INSERT INTO contact VALUES(1,'b','Same','',0,1,'123'),(2,'a','Same','',0,1,NULL),(3,'g@chatroom','Group','',0,1,NULL),(4,'gh_news','News','',8,3,NULL);"
    ).unwrap();
    (temp, SqliteContacts::new(path))
}

#[test]
fn capabilities_and_typed_contact_projection_preserve_identity_and_optional_phone() {
    let (_temp, source) = fixture();
    let directory = source.contacts().unwrap();
    assert!(directory.capabilities.names && directory.capabilities.classification);
    assert!(!directory.capabilities.full_membership && !directory.capabilities.labels);
    assert_eq!(directory.contacts[0].phone.as_deref(), Some("123"));
    assert_eq!(directory.contacts[3].kind, ContactKind::Official);
    let people = domain::list(
        &source,
        ContactQuery {
            text: None,
            view: ContactView::People,
            offset: 0,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(
        people
            .contacts
            .iter()
            .map(|c| c.id.0.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    let legacy = domain::list(
        &source,
        ContactQuery {
            text: None,
            view: ContactView::VisibleDirectory,
            offset: 0,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(
        legacy
            .contacts
            .iter()
            .map(|c| c.id.0.as_str())
            .collect::<Vec<_>>(),
        ["b", "a", "g@chatroom"]
    );
    assert_eq!(domain::resolve(&source, "Same"), Err(Error::Ambiguous));
    assert!(matches!(
        source.tags(),
        Err(Error::Unsupported("contact labels"))
    ));
}

#[test]
fn member_schema_variants_and_empty_complete_groups_are_detected() {
    for name in ["username", "chat_room_name", "name"] {
        let (_temp, source) = fixture();
        let conn = Connection::open(&source.path).unwrap();
        conn.execute_batch(&format!("CREATE TABLE chat_room(id INTEGER, [{name}] TEXT, owner TEXT); CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER); INSERT INTO chat_room VALUES(7,'g@chatroom','a'); INSERT INTO chatroom_member VALUES(7,1),(7,2);")).unwrap();
        let (group, members) = domain::members(&source, "Group").unwrap();
        assert_eq!(group.id.0, "g@chatroom");
        assert_eq!(members.coverage, MembershipCoverage::Complete);
        assert_eq!(members.members[0].id.0, "a");
        assert!(members.members[0].is_owner);
        conn.execute("DELETE FROM chatroom_member", []).unwrap();
        let members = source.members(&group.id).unwrap();
        assert_eq!(members.coverage, MembershipCoverage::Complete);
        assert!(members.members.is_empty());
    }
}

#[test]
fn broken_member_links_and_duplicate_room_identity_do_not_fall_back() {
    let (_temp, source) = fixture();
    let conn = Connection::open(&source.path).unwrap();
    conn.execute_batch("CREATE TABLE chat_room(id INTEGER,username TEXT,owner TEXT); CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER); INSERT INTO chat_room VALUES(7,'g@chatroom','a'); INSERT INTO chatroom_member VALUES(7,999);").unwrap();
    assert!(matches!(
        source.members(&ContactId("g@chatroom".into())),
        Err(Error::InvalidData(_))
    ));
    conn.execute("INSERT INTO chat_room VALUES(8,'g@chatroom','b')", [])
        .unwrap();
    assert!(matches!(
        source.members(&ContactId("g@chatroom".into())),
        Err(Error::Ambiguous)
    ));
}

#[test]
fn observed_senders_are_explicit_and_unresolved_sender_is_an_error() {
    let (temp, mut source) = fixture();
    let group = ContactId("g@chatroom".into());
    assert!(matches!(
        source.members(&group),
        Err(Error::Unsupported("group membership"))
    ));
    let path = temp.path().join("messages.db");
    let conn = Connection::open(&path).unwrap();
    let table = format!("Msg_{:x}", md5::compute(group.0.as_bytes()));
    conn.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(1,'a'); CREATE TABLE [{table}](real_sender_id INTEGER); INSERT INTO [{table}] VALUES(1),(1);")).unwrap();
    source.message_paths.push(path);
    let members = source.members(&group).unwrap();
    assert_eq!(members.coverage, MembershipCoverage::ObservedSenders);
    assert_eq!(members.members.len(), 1);
    conn.execute(&format!("INSERT INTO [{table}] VALUES(999)"), [])
        .unwrap();
    assert!(matches!(source.members(&group), Err(Error::InvalidData(_))));
}

#[test]
fn missing_schema_invalid_values_and_duplicate_ids_are_not_successful_empty_results() {
    let (_temp, source) = fixture();
    let conn = Connection::open(&source.path).unwrap();
    conn.execute(
        "INSERT INTO contact(username,nick_name,remark) VALUES('a','Other','')",
        [],
    )
    .unwrap();
    assert!(matches!(source.contacts(), Err(Error::Ambiguous)));
    let legacy = SqliteContacts::legacy_directory(source.path.clone());
    assert_eq!(
        legacy
            .contacts()
            .unwrap()
            .contacts
            .iter()
            .filter(|contact| contact.id.0 == "a")
            .count(),
        2
    );
    assert_eq!(domain::resolve(&legacy, "a"), Err(Error::Ambiguous));
    conn.execute("DELETE FROM contact WHERE nick_name='Other'", [])
        .unwrap();
    conn.execute("UPDATE contact SET nick_name=x'ff' WHERE username='a'", [])
        .unwrap();
    assert!(matches!(source.contacts(), Err(Error::InvalidData(_))));
    conn.execute_batch("DROP TABLE contact; CREATE TABLE contact(username TEXT);")
        .unwrap();
    assert!(matches!(
        source.contacts(),
        Err(Error::Unsupported("contact names"))
    ));
}

#[test]
fn labels_reuse_bounded_blob_parser_and_return_domain_ids() {
    let (_temp, source) = fixture();
    let conn = Connection::open(&source.path).unwrap();
    conn.execute_batch("ALTER TABLE contact ADD COLUMN extra_buffer BLOB; CREATE TABLE contact_label(label_id_,label_name_,sort_order_); INSERT INTO contact_label VALUES(7,'Friends',1); UPDATE contact SET extra_buffer=x'f2010137' WHERE username='a';").unwrap();
    let tags = source.tags().unwrap();
    assert_eq!(tags[0].name, "Friends");
    assert_eq!(tags[0].members[0].id.0, "a");
    assert!(source.contacts().unwrap().capabilities.labels);
}

#[test]
fn label_source_distinguishes_unavailable_unsupported_malformed_and_budget() {
    let (temp, source) = fixture();
    let missing = SqliteContacts::new(temp.path().join("missing.db"));
    assert_eq!(missing.tags(), Err(Error::Unavailable));
    assert!(!missing.path.exists());
    let conn = Connection::open(&source.path).unwrap();
    conn.execute_batch("CREATE TABLE contact_label(label_id_,label_name_,sort_order_); INSERT INTO contact_label VALUES(7,'Friends',1);").unwrap();
    assert_eq!(
        source.tags(),
        Err(Error::Unsupported("contact label membership"))
    );
    conn.execute_batch("ALTER TABLE contact ADD COLUMN extra_buffer BLOB; UPDATE contact SET extra_buffer='not a binary buffer' WHERE username='a';").unwrap();
    assert!(matches!(source.tags(), Err(Error::InvalidData(_))));
    conn.execute(
        "UPDATE contact SET extra_buffer=? WHERE username='a'",
        [vec![0u8; MAX_BUFFER_BYTES + 1]],
    )
    .unwrap();
    assert_eq!(source.tags(), Err(Error::Limit));
    assert_eq!(
        read_tags(&source.path, &HashMap::new())
            .unwrap_err()
            .to_string(),
        "contact buffer byte limit exceeded"
    );
}

#[test]
fn classification_is_reported_unsupported_without_hiding_directory_entries() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unclassified.db");
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT); INSERT INTO contact VALUES('a','Person','');").unwrap();
    let source = SqliteContacts::new(path);
    assert!(!source.contacts().unwrap().capabilities.classification);
    assert!(matches!(
        domain::list(
            &source,
            ContactQuery {
                text: None,
                view: ContactView::People,
                offset: 0,
                limit: 1
            }
        ),
        Err(Error::Unsupported("contact classification"))
    ));
    assert_eq!(
        domain::list(
            &source,
            ContactQuery {
                text: None,
                view: ContactView::VisibleDirectory,
                offset: 0,
                limit: 1
            }
        )
        .unwrap()
        .total,
        1
    );
}

#[test]
fn member_owner_text_is_bounded_before_copying() {
    let (_temp, source) = fixture();
    let conn = Connection::open(&source.path).unwrap();
    conn.execute_batch("CREATE TABLE chat_room(id INTEGER,username TEXT,owner TEXT); CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER);").unwrap();
    conn.execute(
        "INSERT INTO chat_room VALUES(7,'g@chatroom',?)",
        ["x".repeat(4097)],
    )
    .unwrap();
    assert!(matches!(
        source.members(&ContactId("g@chatroom".into())),
        Err(Error::Limit)
    ));
}

#[test]
fn separate_sqlite_sources_with_the_same_contact_id_remain_account_scoped() {
    let (_first, first) = fixture();
    let (_second, second) = fixture();
    Connection::open(&first.path)
        .unwrap()
        .execute(
            "UPDATE contact SET remark='First account' WHERE username='a'",
            [],
        )
        .unwrap();
    Connection::open(&second.path)
        .unwrap()
        .execute(
            "UPDATE contact SET remark='Second account' WHERE username='a'",
            [],
        )
        .unwrap();
    assert_eq!(
        domain::resolve(&first, "a").unwrap().display(),
        "First account"
    );
    assert_eq!(
        domain::resolve(&second, "a").unwrap().display(),
        "Second account"
    );
}

#[test]
fn cached_directory_preserves_snapshot_scope_names_and_classification() {
    let mut names = HashMap::from([
        ("a".into(), "Cached remark".into()),
        ("g@chatroom".into(), "Group".into()),
        ("gh_public".into(), "Public".into()),
        ("wxid_verified".into(), "Verified service".into()),
    ]);
    let source = cached_directory(&names, &HashMap::from([("wxid_verified".into(), 8)]));
    names.insert("a".into(), "New remark".into());
    names.insert("new_contact".into(), "New contact".into());
    let page = domain::list(
        &source,
        ContactQuery {
            text: None,
            view: ContactView::People,
            offset: 0,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.contacts[0].id.0, "a");
    assert_eq!(page.contacts[0].display(), "Cached remark");
    assert!(page.contacts[0].nickname.is_none() && page.contacts[0].remark.is_none());
    assert_eq!(
        domain::list(
            &source,
            ContactQuery {
                text: Some("New"),
                view: ContactView::People,
                offset: 0,
                limit: 10
            }
        )
        .unwrap()
        .total,
        0
    );
}

#[test]
fn connection_display_names_preserve_empty_and_caller_transaction_semantics() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT)")
        .unwrap();
    assert!(display_names(&conn).unwrap().is_empty());
    let tx = conn.transaction().unwrap();
    tx.execute_batch("INSERT INTO contact VALUES('a','Nick','Remark'),('b','Nickname',NULL),('c',NULL,''),('a','Latest','');").unwrap();
    assert_eq!(
        display_names(&tx).unwrap(),
        BTreeMap::from([
            ("a".into(), "Latest".into()),
            ("b".into(), "Nickname".into()),
            ("c".into(), "c".into()),
        ])
    );
    tx.rollback().unwrap();
    assert!(display_names(&conn).unwrap().is_empty());
    conn.execute("INSERT INTO contact VALUES('a',x'ff',NULL)", [])
        .unwrap();
    assert!(matches!(display_names(&conn), Err(Error::InvalidData(_))));
}
