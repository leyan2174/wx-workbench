use super::*;
use domain::Kind;

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
