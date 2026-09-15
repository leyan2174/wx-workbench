use super::*;
use domain::Kind;

#[test]
fn raw_projection_keeps_sender_resolution_and_unused_sort_scalar_validation() {
    for size in [1, 4097] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.db");
        let table = layout::table_for_username("synthetic-peer");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE Name2Id(user_name TEXT);
             INSERT INTO Name2Id VALUES('synthetic-sender');
             CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,
             real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER,sort_seq TEXT);"
        )).unwrap();
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(1,1,1,1,'synthetic',0,?1)"),
            ["x".repeat(size)],
        )
        .unwrap();
        drop(conn);
        let snapshot = Snapshot::open(
            vec![SourceFile {
                logical_name: "message/message_0.db".into(),
                path,
                kind: SourceKind::Ordinary,
            }],
            ["synthetic-peer".into()],
        )
        .unwrap();
        let reference = snapshot
            .resolve(
                &MessageSelector {
                    username: "synthetic-peer",
                    local_id: 1,
                    timestamp: Some(1),
                },
                SourceKind::Ordinary,
            )
            .unwrap();
        let result = snapshot.read_metadata(reference.evidence());
        if size <= 4096 {
            assert_eq!(result.unwrap().sender.as_deref(), Some("synthetic-sender"));
        } else {
            let error = result.unwrap_err();
            assert!(matches!(
                error.downcast_ref::<domain::Error>(),
                Some(domain::Error::Limit)
            ));
        }
    }
}

#[test]
fn legacy_wire_selection_is_not_widened_to_business_kinds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("synthetic.db");
    let table = layout::table_for_username("synthetic-peer");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE [{table}](local_id INTEGER, local_type INTEGER, create_time INTEGER,
         real_sender_id INTEGER, message_content TEXT, WCDB_CT_message_content INTEGER)"
    ))
    .unwrap();
    let packed_video = (1_i64 << 32) | 43;
    let packed_system = (1_i64 << 32) | 10000;
    let packed_link = (5_i64 << 32) | 49;
    let packed_file = (6_i64 << 32) | 49;
    let packed_unknown = (1_i64 << 32) | i64::from(u32::MAX);
    for (index, kind) in [
        43,
        62,
        10000,
        10002,
        49,
        packed_link,
        packed_file,
        -1,
        packed_video,
        47,
        48,
        42,
        packed_system,
        packed_unknown,
    ]
    .into_iter()
    .enumerate()
    {
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(?1,?2,?1,NULL,'synthetic',0)"),
            rusqlite::params![index as i64 + 1, kind],
        )
        .unwrap();
    }
    drop(conn);
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path,
            kind: SourceKind::Ordinary,
        }],
        ["synthetic-peer".into()],
    )
    .unwrap();
    let select = |kinds: Vec<Kind>, local_types: Vec<i64>| {
        let rows = snapshot
            .read_legacy_page(
                0,
                &domain::Filter {
                    kinds,
                    ..Default::default()
                },
                &LegacyReadPolicy { local_types },
                100,
                true,
            )
            .unwrap();
        rows.into_iter()
            .map(|row| row.local_type)
            .collect::<BTreeSet<_>>()
    };
    let expected = |values: &[i64]| values.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(select(vec![], vec![43]), expected(&[43, packed_video]));
    assert_eq!(
        select(vec![Kind::Video], vec![]),
        expected(&[43, 62, packed_video])
    );
    assert_eq!(
        select(vec![], vec![10000]),
        expected(&[10000, packed_system])
    );
    assert_eq!(
        select(vec![Kind::System], vec![]),
        expected(&[10000, 10002, packed_system])
    );
    assert_eq!(
        select(vec![], vec![49]),
        expected(&[49, packed_link, packed_file])
    );
    assert_eq!(
        select(vec![Kind::Structured], vec![]),
        expected(&[49, packed_link, packed_file])
    );
    assert_eq!(select(vec![], vec![packed_file]), expected(&[packed_file]));
    assert_eq!(select(vec![], vec![-1]), expected(&[-1]));
    assert_eq!(
        select(vec![], vec![i64::from(u32::MAX)]),
        expected(&[-1, packed_unknown])
    );
    assert_eq!(select(vec![], vec![47, 48, 42]), expected(&[47, 48, 42]));
    assert_eq!(
        select(vec![Kind::Video], vec![43]),
        expected(&[43, packed_video])
    );
}
