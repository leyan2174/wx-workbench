use super::*;

fn table() -> String {
    format!("Msg_{:x}", md5::compute("synthetic-peer"))
}

#[test]
fn optional_body_columns_preserve_counts_nulls_text_lengths_and_closed_range() {
    for (compressed, packed) in [(false, false), (true, false), (false, true), (true, true)] {
        let conn = Connection::open_in_memory().unwrap();
        let table = table();
        let extra = format!(
            "{}{}",
            if compressed {
                ", compress_content BLOB"
            } else {
                ""
            },
            if packed {
                ", packed_info_data TEXT"
            } else {
                ""
            }
        );
        conn.execute_batch(&format!("CREATE TABLE [{table}](create_time INTEGER,message_content TEXT{extra});
            INSERT INTO [{table}](create_time,message_content) VALUES(10,'a中'),(20,NULL),(30,'outside');")).unwrap();
        if compressed {
            conn.execute_batch(&format!(
                "UPDATE [{table}] SET compress_content=X'010203' WHERE create_time=10;"
            ))
            .unwrap();
        }
        if packed {
            conn.execute_batch(&format!(
                "UPDATE [{table}] SET packed_info_data='four' WHERE create_time=10;"
            ))
            .unwrap();
        }
        let result = query_message_table_plan_stats(
            &conn,
            &table,
            TimeRange {
                start: Some(10),
                end: Some(20),
            },
        )
        .unwrap();
        assert_eq!(result.message_count, 2);
        assert_eq!((result.first_ts, result.last_ts), (Some(10), Some(20)));
        assert_eq!(
            result.message_body_bytes,
            2 + if compressed { 3 } else { 0 } + if packed { 4 } else { 0 }
        );
        let empty = query_message_table_plan_stats(
            &conn,
            &table,
            TimeRange {
                start: Some(21),
                end: Some(29),
            },
        )
        .unwrap();
        assert_eq!(empty.message_count, 0);
        assert_eq!(empty.message_body_bytes, 0);
        assert_eq!((empty.first_ts, empty.last_ts), (None, None));
    }
}

#[test]
fn required_columns_and_bad_database_remain_explicit_partial_failures() {
    for schema in [
        "create_time INTEGER",
        "message_content TEXT",
        "invalid database",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("message.db");
        if schema == "invalid database" {
            fs::write(&path, b"synthetic-not-sqlite").unwrap();
        } else {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("CREATE TABLE [{}]({schema});", table()))
                .unwrap();
            assert!(query_message_table_plan_stats(&conn, &table(), TimeRange::default()).is_err());
        }
        let inputs = PlanDatabases {
            message: vec!["message.db".into()],
            ..Default::default()
        };
        let mut source = SqliteSource::new(&root.path().canonicalize().unwrap(), &inputs).unwrap();
        let chats = vec![PlanChat {
            index: 1,
            username: "synthetic-peer".into(),
            chat_name: "synthetic".into(),
            chat_type: "single".into(),
        }];
        let plan = domain::Plan::read(&mut source, &chats, TimeRange::default()).unwrap();
        assert_eq!(plan.rows[0].messages.message_count, 0);
        assert!(plan.rows[0]
            .statuses
            .contains(&domain::Partial::MessageReadFailed));
    }
}
