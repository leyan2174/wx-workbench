//! 消息详细解码共用的定位逻辑：扫描全部已知分片，明确报告 ID 冲突。

use super::*;
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub enum DecodeKind {
    Transfer,
    Location,
}

pub async fn q_decode(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    kind: DecodeKind,
) -> Result<Value> {
    let Some(username) = resolve_username(chat, names) else {
        return Ok(failure(1, format!("找不到聊天对象: {chat}")));
    };
    let (resolved, _) = find_msg_shards(db, names, &username).await?;
    // 缓存路径用于读取；原数据库路径用于来源展示，两者不能混用。
    let sources: HashMap<PathBuf, PathBuf> = resolved
        .iter()
        .map(|s| (s.path.clone(), PathBuf::from(&s.rel_key)))
        .collect();
    let shards: Vec<_> = resolved.into_iter().map(|s| (s.path, s.table)).collect();
    if shards.is_empty() {
        return Ok(failure(1, format!("找不到 {chat} 的消息表")));
    }
    tokio::task::spawn_blocking(move || {
        lookup_with_sources(&shards, &sources, &username, local_id, create_time, kind)
    })
    .await?
}

fn failure(exit_code: i32, text: String) -> Value {
    json!({"exit_code":exit_code,"text":text})
}

#[cfg(test)]
fn lookup(
    shards: &[(PathBuf, String)],
    username: &str,
    local_id: i64,
    create_time: i64,
) -> Result<Value> {
    lookup_kind(
        shards,
        username,
        local_id,
        create_time,
        DecodeKind::Transfer,
    )
}

#[cfg(test)]
fn lookup_kind(
    shards: &[(PathBuf, String)],
    username: &str,
    local_id: i64,
    create_time: i64,
    decode_kind: DecodeKind,
) -> Result<Value> {
    lookup_with_sources(
        shards,
        &HashMap::new(),
        username,
        local_id,
        create_time,
        decode_kind,
    )
}

fn lookup_with_sources(
    shards: &[(PathBuf, String)],
    sources: &HashMap<PathBuf, PathBuf>,
    username: &str,
    local_id: i64,
    create_time: i64,
    decode_kind: DecodeKind,
) -> Result<Value> {
    let mut matches = Vec::new();
    for (path, table) in shards {
        anyhow::ensure!(msg_table_re().is_match(table), "消息表名不合法");
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut statement = conn.prepare(&format!("SELECT local_type, create_time, message_content, WCDB_CT_message_content FROM [{table}] WHERE local_id=?1 AND (?2=0 OR create_time=?2)"))?;
        let rows = statement.query_map([local_id, create_time], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                get_content_bytes(row, 2),
                row.get::<_, i64>(3).unwrap_or(0),
            ))
        })?;
        for row in rows {
            matches.push((sources.get(path).unwrap_or(path).clone(), row?));
        }
    }
    if matches.is_empty() {
        return Ok(failure(
            1,
            format!(
                "找不到 local_id={local_id}, create_time={create_time} 的消息（已扫描 {} 个分片）",
                shards.len()
            ),
        ));
    }
    if matches.len() > 1 {
        let details: Vec<_> = matches
            .iter()
            .map(|(path, (_, time, _, _))| {
                format!(
                    "{} create_time={time}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )
            })
            .collect();
        return Ok(failure(
            2,
            format!(
                "local_id={local_id} 有 {} 个匹配，无法唯一定位:\n  {}\n请指定 create_time 时间戳",
                matches.len(),
                details.join("\n  ")
            ),
        ));
    }
    let (path, (kind, time, bytes, compression)) = matches.pop().unwrap();
    let expected_type = match decode_kind {
        DecodeKind::Transfer => 49,
        DecodeKind::Location => 48,
    };
    if kind as u64 & 0xffffffff != expected_type {
        return Ok(failure(
            1,
            format!("消息类型不匹配（local_type={kind}），期望 base_type={expected_type}"),
        ));
    }
    let xml = decompress_message(&bytes, compression);
    let xml = if username.ends_with("@chatroom") {
        strip_group_prefix(&xml)
    } else {
        xml
    };
    if xml.is_empty() {
        return Ok(failure(1, "消息 content 为空或无法解码".into()));
    }
    if let DecodeKind::Location = decode_kind {
        return match crate::message::location::parse(&xml) {
            Some(location) => Ok(
                json!({"exit_code":0,"text":location.render(),"location":location,"username":username,"local_id":local_id,"create_time":time,"source":path.file_name().unwrap_or_default().to_string_lossy()}),
            ),
            None => Ok(failure(
                1,
                "消息是 type=48 但缺有效 <location> 节点 (schema 异常)".into(),
            )),
        };
    }
    match crate::message::transfer::parse(&xml) {
        Ok(transfer) => Ok(
            json!({"exit_code":0,"text":transfer.render(),"transfer":transfer,"username":username,"local_id":local_id,"create_time":time,"source":path.file_name().unwrap_or_default().to_string_lossy()}),
        ),
        Err(error) => Ok(failure(1, error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "wx-transfer-query-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn shard(
            &self,
            name: &str,
            time: i64,
            compressed: bool,
            kind: i64,
            xml: &str,
        ) -> (PathBuf, String) {
            let path = self.0.join(name);
            let table = format!("Msg_{:x}", md5::compute(b"test-person"));
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("CREATE TABLE [{table}] (local_id INTEGER, local_type INTEGER, create_time INTEGER, message_content BLOB, WCDB_CT_message_content INTEGER)")).unwrap();
            let bytes = if compressed {
                zstd::encode_all(xml.as_bytes(), 1).unwrap()
            } else {
                xml.as_bytes().to_vec()
            };
            conn.execute(
                &format!("INSERT INTO [{table}] VALUES (7, ?1, ?2, ?3, ?4)"),
                rusqlite::params![kind, time, bytes, if compressed { 4 } else { 0 }],
            )
            .unwrap();
            (path, table)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const XML: &str = "<msg><appmsg><type>2000</type><title>微信转账</title><wcpayinfo><paysubtype>1</paysubtype><feedesc>¥0.01</feedesc><transferid>test-id</transferid></wcpayinfo></appmsg></msg>";

    #[test]
    fn location_shards_compression_group_and_errors() {
        let fixture = Fixture::new();
        let xml =
            "wxid_demo:\n<msg><location poiname='示例' poiPhone='000' x='31.2' y='121.5'/></msg>";
        let shards = vec![
            fixture.shard("message_0.db", 100, false, 48, xml),
            fixture.shard("message_1.db", 200, true, (1 << 32) | 48, xml),
        ];
        let run = |id, time| {
            lookup_kind(&shards, "demo@chatroom", id, time, DecodeKind::Location).unwrap()
        };
        assert_eq!(run(7, 0)["exit_code"], 2);
        let result = run(7, 200);
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["location"]["poiPhone"], "000");
        assert_eq!(result["location"]["lat"], 31.2);
        assert_eq!(result["source"], "message_1.db");
        assert!(result["text"]
            .as_str()
            .unwrap()
            .contains("经纬度: (31.200000, 121.500000)"));
        assert_eq!(run(99, 0)["exit_code"], 1);
        let invalid = vec![fixture.shard("bad.db", 300, false, 48, "<msg/>")];
        assert_eq!(
            lookup_kind(&invalid, "demo", 7, 300, DecodeKind::Location).unwrap()["exit_code"],
            1
        );
        assert_eq!(
            lookup_kind(&shards, "demo", 7, 200, DecodeKind::Transfer).unwrap()["exit_code"],
            1
        );
    }

    #[test]
    fn requires_timestamp_for_shard_collision_and_decodes_compressed_flagged_type() {
        let fixture = Fixture::new();
        let shards = vec![
            fixture.shard("message_0.db", 100, false, 49, XML),
            fixture.shard("message_1.db", 200, true, (1 << 32) | 49, XML),
        ];
        assert_eq!(
            lookup(&shards, "test-person", 7, 0).unwrap()["exit_code"],
            2
        );
        let result = lookup(&shards, "test-person", 7, 200).unwrap();
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["transfer"]["fee_desc"], "¥0.01");
        assert_eq!(result["source"], "message_1.db");
        assert_eq!(
            lookup(&shards, "test-person", 8, 0).unwrap()["exit_code"],
            1
        );
    }

    #[test]
    fn identical_timestamp_stays_ambiguous_and_wrong_message_type_fails() {
        let fixture = Fixture::new();
        let shards = vec![
            fixture.shard("a.db", 100, false, 49, XML),
            fixture.shard("b.db", 100, false, 49, XML),
        ];
        assert_eq!(
            lookup(&shards, "test-person", 7, 100).unwrap()["exit_code"],
            2
        );
        let wrong = vec![fixture.shard("wrong.db", 100, false, 1, XML)];
        assert_eq!(
            lookup(&wrong, "test-person", 7, 100).unwrap()["exit_code"],
            1
        );
        let unsafe_table = vec![(shards[0].0.clone(), "contact; DROP TABLE contact".into())];
        assert!(lookup(&unsafe_table, "test-person", 7, 0).is_err());
    }

    #[test]
    fn history_uses_transfer_summary_instead_of_generic_link_title() {
        assert_eq!(parse_appmsg(XML).unwrap(), "[转账·发起转账] ¥0.01");
    }
}
