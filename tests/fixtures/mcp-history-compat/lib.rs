// This reference implementation only checks the historical oracle itself.
// Current production queries are exercised by the runtime target and root MCP tests.
#[path = "legacy_selection.rs"]
pub mod history_selection;

#[cfg(test)]
mod tests {
    use super::history_selection::Selection;
    use rusqlite::{types::Value as SqlValue, Connection};
    use serde_json::{json, Value};

    const TABLE: &str = "Msg_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("CREATE TABLE {TABLE}(local_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content)")).unwrap();
        conn
    }

    #[test]
    fn legacy_ast_sqlite_oracle_matches_all_parameter_combinations() {
        let oracle: Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
        let databases: Vec<_> = oracle["shards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rows| {
                let conn = database();
                for row in rows.as_array().unwrap() {
                    conn.execute(
                        &format!("INSERT INTO {TABLE} VALUES(?1,?2,?3,?4,?5,?6)"),
                        rusqlite::params![
                            row[0].as_i64(),
                            row[1].as_i64(),
                            row[2].as_i64(),
                            row[3].as_i64(),
                            row[4].as_str(),
                            row[5].as_i64()
                        ],
                    )
                    .unwrap();
                }
                conn
            })
            .collect();
        for case in oracle["cases"].as_array().unwrap() {
            let types: Vec<_> = case["types"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_i64().unwrap())
                .collect();
            let selection = Selection::new(
                case["limit"].as_u64().unwrap() as usize,
                case["offset"].as_u64().unwrap() as usize,
                case["since"].as_i64(),
                case["until"].as_i64(),
                &types,
                case["oldest"].as_bool().unwrap(),
            )
            .unwrap();
            let mut entries = Vec::new();
            for (shard, conn) in databases.iter().enumerate() {
                entries.extend(
                    selection
                        .query_shard(conn, TABLE, |row| Ok(json!([shard, row.get::<_, i64>(0)?])))
                        .unwrap(),
                );
            }
            assert_eq!(json!(selection.page(entries)), case["expected"], "{case}");
        }
        assert_eq!(oracle["cases"].as_array().unwrap().len(), 288);
    }

    #[test]
    fn base_types_include_all_high_bits_and_full_types_are_exact() {
        let conn = database();
        let flagged = (57_i64 << 32) | 49;
        let signed = i64::MIN | 49;
        for (id, kind) in [49, flagged, signed, 1, (5_i64 << 32) | 1, 3]
            .into_iter()
            .enumerate()
        {
            conn.execute(
                &format!("INSERT INTO {TABLE} VALUES(?1,?2,?1,0,'raw',0)"),
                rusqlite::params![id as i64, kind],
            )
            .unwrap();
        }
        for (types, expected) in [
            (vec![49], vec![0, 1, 2]),
            (vec![flagged], vec![1]),
            (vec![signed], vec![2]),
            (vec![49, flagged, 1, 49], vec![0, 1, 2, 3, 4]),
            (vec![u32::MAX as i64], vec![]),
        ] {
            for oldest in [true, false] {
                let selection = Selection::new(20, 0, None, None, &types, oldest).unwrap();
                let actual = selection.page(
                    selection
                        .query_shard(&conn, TABLE, |row| row.get::<_, i64>(0))
                        .unwrap(),
                );
                assert_eq!(actual, expected);
            }
        }
        // 旧 AST 的 IN(49) 只匹配未带高位的行，不能把本扩展伪称为旧行为。
        let old_count: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {TABLE} WHERE local_type IN (49)"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_count, 1);
    }

    #[test]
    fn mapper_receives_unchanged_sender_content_storage_and_compression() {
        let conn = database();
        let contents = [
            SqlValue::Text("wxid_member:\n原正文".into()),
            SqlValue::Blob(vec![0, 255, 40, 181]),
            SqlValue::Null,
        ];
        for (id, content) in contents.iter().enumerate() {
            conn.execute(
                &format!("INSERT INTO {TABLE} VALUES(?1,49,?1,77,?2,4)"),
                rusqlite::params![id as i64, content],
            )
            .unwrap();
        }
        let selection = Selection::new(10, 0, None, None, &[], false).unwrap();
        let rows = selection.page(
            selection
                .query_shard(&conn, TABLE, |row| {
                    Ok((
                        row.get::<_, i64>(3)?,
                        row.get::<_, SqlValue>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .unwrap(),
        );
        assert_eq!(
            rows,
            contents
                .into_iter()
                .map(|content| (77, content, 4))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn invalid_arguments_identifiers_and_row_errors_are_not_empty_successes() {
        assert!(Selection::new(0, 0, None, None, &[], false).is_err());
        assert!(Selection::new(1, usize::MAX, None, None, &[], false).is_err());
        assert!(Selection::new(1, 0, Some(2), Some(1), &[], false).is_err());
        assert!(Selection::new(1, 0, None, None, &(0..101).collect::<Vec<_>>(), false).is_err());
        let conn = database();
        let selection = Selection::new(1, 0, None, None, &[], false).unwrap();
        for name in [
            "Msg_bad",
            "Msg_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]; DROP TABLE x;--",
            "Msg_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ] {
            assert!(selection.query_shard(&conn, name, |_| Ok(())).is_err());
        }
        conn.execute(&format!("INSERT INTO {TABLE} VALUES(1,1,0,0,'text',0)"), [])
            .unwrap();
        assert!(selection
            .query_shard(&conn, TABLE, |row| row.get::<_, i64>(4))
            .is_err());
    }
}
