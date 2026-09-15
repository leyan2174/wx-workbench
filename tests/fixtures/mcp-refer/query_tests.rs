use super::super::strict_message::{with_resolved, Resolution, MAX_STORED_BYTES};
use super::*;
use crate::daemon::query::encrypted_cache;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

// Public reply endpoint budget, independent of the adapter's private constant.
const MAX_DECODED_BYTES: usize = 131_072;

fn golden() -> Value {
    serde_json::from_str(include_str!("golden.json")).unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    db: DbCache,
    names: Names,
    paths: Vec<PathBuf>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_keys(&["message/message_0.db", "message/message_1.db"]).await
    }

    async fn with_keys(raw_keys: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("wxid_self_abcd/db_storage");
        let cache = root.path().join("cache");
        fs::create_dir_all(source.join("message")).unwrap();
        fs::create_dir(&cache).unwrap();
        let mut keys = HashMap::new();
        let mut mtimes = serde_json::Map::new();
        let mut paths = Vec::new();
        for key in raw_keys {
            let key = (*key).to_owned();
            let original = source.join(&key);
            let path = cache.join(format!("{:x}.db", md5::compute(&key)));
            let conn = encrypted_cache::sqlite(&path);
            for username in [
                "wxid_peer",
                "room@chatroom",
                "wxid_'; DROP TABLE contact;--",
            ] {
                let table = format!("Msg_{:x}", md5::compute(username));
                conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type,create_time,WCDB_CT_message_content,message_content)")).unwrap();
            }
            drop(conn);
            let mt = encrypted_cache::seed(&path, &original);
            mtimes.insert(key.clone(), json!({"db_mt": mt, "wal_mt": 0, "path": path}));
            keys.insert(key, "11".repeat(32));
            paths.push(path);
        }
        let file = cache.join("_mtimes.json");
        fs::write(&file, serde_json::to_vec(&mtimes).unwrap()).unwrap();
        let db = DbCache::with_dirs(source, cache, file, keys).await.unwrap();
        let names = Names {
            map: serde_json::from_value(golden()["names"].clone()).unwrap(),
            msg_db_keys: raw_keys.iter().rev().map(|key| (*key).to_owned()).collect(),
            biz_msg_db_keys: vec![],
            verify_flags: HashMap::new(),
        };
        Self {
            root,
            db,
            names,
            paths,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "测试按原始数据库字段构造消息，用于校验身份冲突与异常列"
    )]
    fn insert(
        &self,
        shard: usize,
        user: &str,
        id: i64,
        time: i64,
        kind: i64,
        body: rusqlite::types::Value,
        compression: i64,
    ) {
        let table = format!("Msg_{:x}", md5::compute(user));
        Connection::open(&self.paths[shard])
            .unwrap()
            .execute(
                &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,?4,?5)"),
                rusqlite::params![id, kind, time, compression, body],
            )
            .unwrap();
    }

    async fn query(&self, user: &str, id: i64, time: i64) -> Result<Value> {
        q_decode_refer(&self.db, &self.names, user, id, time).await
    }
}

fn body() -> String {
    golden()["cases"][0]["xml"].as_str().unwrap().into()
}

#[tokio::test]
async fn shared_lookup_supports_attachment_decode_limits() {
    let f = Fixture::new().await;
    for (index, body) in [
        rusqlite::types::Value::Blob(zstd::encode_all("x".repeat(500_000).as_bytes(), 1).unwrap()),
        rusqlite::types::Value::Text("x".repeat(500_000)),
    ]
    .into_iter()
    .enumerate()
    {
        f.insert(0, "wxid_peer", index as i64, 123, 49, body, 4);
        let Resolution::Found(message) = with_resolved(
            &f.db,
            &f.names,
            "wxid_peer",
            index as i64,
            0,
            move |snapshot, raw| {
                assert_eq!(
                    snapshot.conversation(&raw.reference)?,
                    &crate::business::messages::Conversation::Known("wxid_peer".into())
                );
                assert_eq!(raw.logical_source, "message/message_0.db");
                assert_eq!(raw.local_id, Some(index as i64));
                assert_eq!(raw.timestamp, 123);
                assert_eq!(raw.local_type, 49);
                Ok(raw.detached_content())
            },
        )
        .await
        .unwrap() else {
            panic!("expected unique message");
        };
        assert_eq!(
            message.bounded_decode(500_000).unwrap(),
            vec![b'x'; 500_000]
        );
        assert!(message.bounded_decode(499_999).is_err());
        assert!(message.bounded_decode(MAX_DECODED_BYTES).is_err());
    }
}

#[tokio::test]
async fn ambiguous_lookup_does_not_hide_later_shard_errors() {
    let f = Fixture::with_keys(&[
        "message/message_0.db",
        "message/message_1.db",
        "message/message_2.db",
    ])
    .await;
    f.insert(0, "wxid_peer", 7, 123, 1, text("not a reply"), 0);
    f.insert(1, "wxid_peer", 7, 123, 49, text(&body()), 0);
    assert!(matches!(
        with_resolved(&f.db, &f.names, "wxid_peer", 7, 0, |_, _| Ok(()))
            .await
            .unwrap(),
        Resolution::AmbiguousMessage
    ));
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    Connection::open(&f.paths[2])
        .unwrap()
        .execute_batch(&format!(
            "DROP TABLE [{table}]; CREATE VIEW [{table}] AS SELECT 1"
        ))
        .unwrap();
    assert!(
        with_resolved(&f.db, &f.names, "wxid_peer", 7, 0, |_, _| Ok(()))
            .await
            .is_err()
    );
}
fn text(value: &str) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.into())
}

#[tokio::test]
async fn vendor_ast_cases_match_structured_fields_and_rendered_text() {
    let f = Fixture::new().await;
    let g = golden();
    for (index, case) in g["cases"].as_array().unwrap().iter().enumerate() {
        let id = index as i64 + 1;
        let xml = case["xml"].as_str().unwrap();
        let blob = index % 2 == 1;
        let content = if blob {
            rusqlite::types::Value::Blob(zstd::encode_all(xml.as_bytes(), 1).unwrap())
        } else {
            text(xml)
        };
        f.insert(
            index % 2,
            case["username"].as_str().unwrap(),
            id,
            100,
            (57i64 << 32) | 49,
            content,
            4,
        );
    }
    let before: Vec<_> = f.paths.iter().map(|p| fs::read(p).unwrap()).collect();
    for (index, case) in g["cases"].as_array().unwrap().iter().enumerate() {
        let result = f
            .query(case["chat"].as_str().unwrap(), index as i64 + 1, 0)
            .await
            .unwrap();
        assert_eq!(result["exit_code"], 0, "{result}");
        assert_eq!(result["refer"], case["expected"], "{}", case["name"]);
        assert_eq!(result["text"], case["text"], "{}", case["name"]);
        let mut fields: Vec<_> = result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "create_time",
                "exit_code",
                "local_id",
                "refer",
                "source",
                "text",
                "username"
            ]
        );
        assert_eq!(result["username"], case["username"]);
        assert_eq!(result["local_id"], index as i64 + 1);
        assert_eq!(result["create_time"], 100);
        assert_eq!(
            result["source"],
            format!("message/message_{}.db", index % 2)
        );
        assert!(!result.to_string().contains("SECRET"));
    }
    assert_eq!(
        before,
        f.paths
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn zero_is_no_filter_and_duplicates_remain_ambiguous_before_type_filtering() {
    let f = Fixture::new().await;
    f.insert(0, "wxid_peer", 7, 0, 49, text(&body()), 0);
    assert_eq!(f.query("wxid_peer", 7, 0).await.unwrap()["create_time"], 0);
    f.insert(1, "wxid_peer", 7, 100, 1, text("not a reply"), 0);
    assert_eq!(f.query("wxid_peer", 7, 0).await.unwrap()["exit_code"], 2);
    assert_eq!(f.query("wxid_peer", 7, 100).await.unwrap()["exit_code"], 1);
    f.insert(0, "wxid_peer", 8, 200, 49, text(&body()), 0);
    assert_eq!(
        f.query("wxid_peer", 8, 0).await.unwrap()["create_time"],
        200
    );
    f.insert(1, "wxid_peer", 8, 200, 49, text(&body()), 0);
    assert_eq!(f.query("wxid_peer", 8, 200).await.unwrap()["exit_code"], 2);
    f.insert(0, "wxid_peer", 9, 300, 49, text(&body()), 0);
    f.insert(0, "wxid_peer", 9, 300, 49, text(&body()), 0);
    assert_eq!(f.query("wxid_peer", 9, 300).await.unwrap()["exit_code"], 2);
}

#[tokio::test]
async fn explicit_caches_isolate_accounts_and_sql_parameters_preserve_i64() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    a.insert(0, "wxid_peer", i64::MAX, i64::MIN, 49, text(&body()), 0);
    b.insert(
        0,
        "wxid_peer",
        i64::MAX,
        i64::MIN,
        49,
        text(&body().replace("回复", "另一个账号")),
        0,
    );
    let first = a.query("合成联系人", i64::MAX, i64::MIN).await.unwrap();
    let second = b.query("wxid_peer", i64::MAX, i64::MIN).await.unwrap();
    assert_eq!(first["local_id"], i64::MAX);
    assert_eq!(first["create_time"], i64::MIN);
    assert_ne!(first["refer"]["reply_text"], second["refer"]["reply_text"]);
    let user = "wxid_'; DROP TABLE contact;--";
    a.insert(0, user, i64::MIN, -1, 49, text(&body()), 0);
    assert_eq!(a.query(user, i64::MIN, -1).await.unwrap()["exit_code"], 0);
    assert_eq!(a.query("missing name", 1, 0).await.unwrap()["exit_code"], 1);
    assert_eq!(a.query("  ", 1, 0).await.unwrap()["exit_code"], 1);
    assert_eq!(a.query("wxid_peer", 1, 0).await.unwrap()["exit_code"], 1);
    assert_eq!(
        a.query("wxid_peer", i64::MAX, -2).await.unwrap()["exit_code"],
        1
    );
}

#[tokio::test]
async fn malformed_types_compression_and_xml_never_leak_payload() {
    let f = Fixture::new().await;
    let malformed = [
        (1, text(&body()), 0),
        (-4_294_967_247, text(&body()), 0),
        (
            49,
            text(&body().replace("<type>57</type>", "<type>6</type>")),
            0,
        ),
        (49, text("<msg><appmsg><type>57</type></appmsg></msg>"), 0),
        (
            49,
            text("<!DOCTYPE msg [<!ENTITY x 'SECRET'>]><msg>&x;</msg>"),
            0,
        ),
        (49, text("<msg>SECRET"), 0),
        (49, text(&"测".repeat(20_001)), 0),
        (
            49,
            rusqlite::types::Value::Blob(b"SECRET invalid zstd".to_vec()),
            4,
        ),
        (
            49,
            rusqlite::types::Value::Blob(
                zstd::encode_all("x".repeat(MAX_DECODED_BYTES + 1).as_bytes(), 1).unwrap(),
            ),
            4,
        ),
        (49, rusqlite::types::Value::Null, 0),
    ];
    for (index, (kind, content, compression)) in malformed.into_iter().enumerate() {
        let id = index as i64;
        f.insert(0, "wxid_peer", id, 100, kind, content, compression);
        let value = f.query("wxid_peer", id, 100).await.unwrap();
        assert_eq!(value["exit_code"], 1, "{value}");
        assert!(value.get("refer").is_none());
        assert!(!value.to_string().contains("SECRET"));
    }
    f.insert(
        0,
        "wxid_peer",
        100,
        100,
        49,
        rusqlite::types::Value::Integer(57),
        0,
    );
    assert!(f.query("wxid_peer", 100, 100).await.is_err());
    f.insert(
        0,
        "wxid_peer",
        101,
        100,
        49,
        text(&"x".repeat(MAX_STORED_BYTES + 1)),
        0,
    );
    assert!(f.query("wxid_peer", 101, 100).await.is_err());
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    Connection::open(&f.paths[0])
        .unwrap()
        .execute(
            &format!("UPDATE [{table}] SET local_type='49' WHERE local_id=100"),
            [],
        )
        .unwrap();
    assert!(f.query("wxid_peer", 100, 100).await.is_err());
}

#[tokio::test]
async fn unavailable_and_unknown_shards_cannot_turn_into_unique_success() {
    let mut f = Fixture::new().await;
    f.insert(0, "wxid_peer", 1, 100, 49, text(&body()), 0);
    f.names.msg_db_keys.push("message/message_0.db".into());
    assert_eq!(f.query("wxid_peer", 1, 0).await.unwrap()["exit_code"], 0);
    fs::remove_file(f.db.db_dir().join("message/message_1.db")).unwrap();
    assert!(f
        .query("wxid_peer", 1, 0)
        .await
        .unwrap_err()
        .to_string()
        .contains("unavailable"));
    fs::write(
        f.db.db_dir().join("message/message_9.db"),
        b"unknown synthetic shard",
    )
    .unwrap();
    assert!(f
        .query("wxid_peer", 1, 0)
        .await
        .unwrap_err()
        .to_string()
        .contains("unknown"));
    assert!(f.root.path().exists());
}

#[test]
fn decimal_and_xml_shape_boundaries() {
    assert_eq!(integer(" +5_7 "), Some(57));
    for bad in ["5__7", "_57", "57_", "57.0", "999999999999999999999999999"] {
        assert_eq!(integer(bad), None);
    }
    let names = HashMap::new();
    for body in [
        "<appmsg><type>57</type><refermsg/></appmsg>",
        "<msg xmlns='untrusted'><appmsg><type>57</type><refermsg/></appmsg></msg>",
    ] {
        assert!(parse_refer(body, "u", "u", "", &names).is_err());
    }
    assert!(parse_refer(
        &body().replace("<type>57</type>", "<type>+5_7</type>"),
        "u",
        "u",
        "",
        &names
    )
    .is_ok());
}

#[tokio::test]
async fn corrupt_shard_empty_tables_and_sqlite_scalar_types_are_explicit() {
    let f = Fixture::new().await;
    assert_eq!(f.query("合成联系", 1, 0).await.unwrap()["exit_code"], 1);
    f.insert(0, "wxid_peer", 1, 100, 49, text(&body()), 0);
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    let conn = Connection::open(&f.paths[0]).unwrap();
    conn.execute(&format!("UPDATE [{table}] SET create_time='100'"), [])
        .unwrap();
    assert!(f.query("wxid_peer", 1, 0).await.is_err());
    conn.execute(
        &format!("UPDATE [{table}] SET create_time=100,WCDB_CT_message_content=4.5"),
        [],
    )
    .unwrap();
    assert!(f.query("wxid_peer", 1, 0).await.is_err());
    conn.execute(
        &format!("UPDATE [{table}] SET WCDB_CT_message_content=NULL"),
        [],
    )
    .unwrap();
    assert_eq!(f.query("合成联系", 1, 100).await.unwrap()["exit_code"], 0);
    conn.execute(
        &format!("UPDATE [{table}] SET message_content=CAST(x'ff' AS TEXT)"),
        [],
    )
    .unwrap();
    assert!(f.query("wxid_peer", 1, 100).await.is_err());
    conn.execute(
        &format!("UPDATE [{table}] SET message_content=?1"),
        [&body()],
    )
    .unwrap();
    drop(conn);
    fs::write(&f.paths[1], b"corrupt synthetic SQLite").unwrap();
    // Prevent recovery from hiding the intentionally unreadable shard.
    let source = f.db.db_dir().join("message/message_1.db");
    let mut bytes = fs::read(&source).unwrap();
    bytes[4032] ^= 1;
    fs::write(source, bytes).unwrap();
    assert!(f.query("wxid_peer", 1, 100).await.is_err());
}

#[test]
fn timestamp_metadata_and_large_server_ids_are_not_lossy_numbers() {
    let names = HashMap::new();
    for value in [
        "0",
        "-1",
        "9223372036854775807",
        "18446744073709551615",
        "invalid",
    ] {
        let xml = body().replace(
            "<createtime>0</createtime>",
            &format!("<createtime>{value}</createtime>"),
        );
        let refer = parse_refer(&xml, "wxid_peer", "peer", "", &names).unwrap();
        assert_eq!(refer["refer_createtime"], value);
        assert_eq!(refer["refer_svrid"], "18446744073709551615");
        assert!(!render(&refer).is_empty());
    }
}

#[tokio::test]
async fn chat_resolution_requires_unique_names_and_prioritizes_exact_username() {
    let mut f = Fixture::new().await;
    f.insert(0, "wxid_peer", 1, 100, 49, text(&body()), 0);
    f.names.map.insert("wxid_peer".into(), "Alpha".into());
    f.names
        .map
        .insert("wxid_shadow".into(), "ALPHA longer".into());
    let result = f.query("aLpHa", 1, 100).await.unwrap();
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["username"], "wxid_peer");
    assert_eq!(f.query("Alph", 1, 100).await.unwrap()["exit_code"], 2);
    f.names.map.insert("wxid_duplicate".into(), "aLPHa".into());
    assert_eq!(f.query("ALPHA", 1, 100).await.unwrap()["exit_code"], 2);
    f.names.map.insert("wxid_shadow".into(), "wxid_peer".into());
    f.names
        .map
        .insert("wxid_duplicate".into(), "WXID_PEER".into());
    let result = f.query("wxid_peer", 1, 100).await.unwrap();
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["username"], "wxid_peer");
    f.names.map.insert("wxid_peer".into(), "唯一昵称".into());
    assert_eq!(f.query("唯一", 1, 100).await.unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn ambiguous_or_missing_chat_returns_before_any_database_access() {
    let mut f = Fixture::new().await;
    f.names.map.insert("wxid_peer".into(), "Same".into());
    f.names.map.insert("wxid_other".into(), "sAME".into());
    // 精确账号查询会因缺失分片失败；昵称歧义必须先返回业务结果，不能触发缓存读取。
    for key in &f.names.msg_db_keys {
        fs::remove_file(f.db.db_dir().join(key)).unwrap();
    }
    let before: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
    for chat in ["same", "sam"] {
        let result = f.query(chat, 1, 100).await.unwrap();
        assert_eq!(result["exit_code"], 2);
        assert!(result.get("refer").is_none());
    }
    for chat in ["not present", "", "  "] {
        assert_eq!(f.query(chat, 1, 100).await.unwrap()["exit_code"], 1);
    }
    assert!(f.query("wxid_peer", 1, 100).await.is_err());
    assert_eq!(
        before,
        f.paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn new_shard_after_sqlite_read_is_rejected_before_returning_success() {
    use std::{
        future::Future,
        sync::{mpsc, Arc},
        task::{Context, Poll, Wake, Waker},
        time::Duration,
    };
    struct Completed(mpsc::Sender<()>);
    impl Wake for Completed {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let f = Fixture::new().await;
        f.insert(0, "wxid_peer", 1, 100, 49, text(&body()), 0);
        assert_eq!(f.query("wxid_peer", 1, 100).await.unwrap()["exit_code"], 0);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let waker = Waker::from(Arc::new(Completed(done_tx)));
        let mut context = Context::from_waker(&waker);
        let mut query = Box::pin(f.query("wxid_peer", 1, 100));
        // 缓存命中后，唯一阻塞线程被闸门占用，保证第一次 poll 停在查询的 JoinHandle。
        assert!(matches!(query.as_mut().poll(&mut context), Poll::Pending));
        release_tx.send(()).unwrap();
        // 收到查询完成唤醒，但暂不恢复外层 future；此刻 SQLite 读取已结束。
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        fs::write(
            f.db.db_dir().join("message/message_9.db"),
            b"new synthetic shard",
        )
        .unwrap();
        let result = query.await;
        gate.await.unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown message shards"));
    });
}

#[tokio::test]
async fn strict_inventory_accepts_known_raw_case_and_rejects_unknown_uppercase() {
    let f = Fixture::with_keys(&["MESSAGE\\MESSAGE_0.DB", "message/message_1.db"]).await;
    f.insert(0, "wxid_peer", 7, 100, 49, text(&body()), 0);
    let before: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let result = f.query("wxid_peer", 7, 100).await.unwrap();
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["source"], "MESSAGE\\MESSAGE_0.DB");
    let unknown = f.db.db_dir().join("message/MESSAGE_9.DB");
    let c = Connection::open(&unknown).unwrap();
    c.execute_batch("CREATE TABLE Synthetic(value INTEGER)")
        .unwrap();
    drop(c);
    let unknown_before = fs::read(&unknown).unwrap();
    let error = f.query("wxid_peer", 7, 100).await.unwrap_err();
    assert!(format!("{error:#}").contains("unknown message shards"));
    assert_eq!(unknown_before, fs::read(unknown).unwrap());
    assert_eq!(
        before,
        f.paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn missing_inventory_directory_is_an_error_not_empty_success() {
    let f = Fixture::new().await;
    f.insert(0, "wxid_peer", 7, 100, 49, text(&body()), 0);
    let before: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let directory = f.db.db_dir().join("message");
    let held = f.root.path().join("held-message");
    fs::rename(&directory, &held).unwrap();
    let originals: Vec<_> = (0..2)
        .map(|n| fs::read(held.join(format!("message_{n}.db"))).unwrap())
        .collect();
    assert!(f.query("wxid_peer", 7, 100).await.is_err());
    assert!(!directory.exists());
    assert_eq!(
        originals,
        (0..2)
            .map(|n| fs::read(held.join(format!("message_{n}.db"))).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        before,
        f.paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn target_view_virtual_and_without_rowid_are_not_absent_tables() {
    for mode in ["view", "virtual", "without-rowid"] {
        let f = Fixture::new().await;
        f.insert(0, "wxid_peer", 7, 100, 49, text(&body()), 0);
        let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
        let c = Connection::open(&f.paths[1]).unwrap();
        c.execute_batch(&format!("ALTER TABLE [{table}] RENAME TO PreservedRows"))
            .unwrap();
        let sql = match mode {
            "view" => format!("CREATE VIEW [{table}] AS SELECT * FROM PreservedRows"),
            "virtual" => {
                // 当前构建没有 FTS 模块；模拟从含扩展的 SQLite 构建获得的虚拟表目录。
                c.execute_batch("PRAGMA writable_schema=ON").unwrap();
                c.execute("INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql) VALUES('table',?1,?1,0,?2)", rusqlite::params![table, format!("CREATE VIRTUAL TABLE [{table}] USING fts5(local_id,local_type,create_time,WCDB_CT_message_content,message_content)")]).unwrap();
                "PRAGMA writable_schema=OFF".into()
            },
            _ => format!("CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY,local_type,create_time,WCDB_CT_message_content,message_content) WITHOUT ROWID"),
        };
        c.execute_batch(&sql).unwrap();
        drop(c);
        let before: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
        let error = f.query("wxid_peer", 7, 100).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("unsupported message table schema"),
            "{mode}: {error:#}"
        );
        assert_eq!(
            before,
            f.paths
                .iter()
                .map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn absent_target_can_be_skipped_and_ordinary_case_variant_still_works() {
    let f = Fixture::new().await;
    f.insert(0, "wxid_peer", 7, 100, 49, text(&body()), 0);
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    Connection::open(&f.paths[1])
        .unwrap()
        .execute_batch(&format!("DROP TABLE [{table}]"))
        .unwrap();
    let c = Connection::open(&f.paths[0]).unwrap();
    c.execute_batch(&format!(
        "ALTER TABLE [{table}] RENAME TO CaseRename; ALTER TABLE CaseRename RENAME TO [{}]",
        table.to_ascii_uppercase()
    ))
    .unwrap();
    drop(c);
    let before: Vec<_> = f.paths.iter().map(|path| fs::read(path).unwrap()).collect();
    assert_eq!(f.query("wxid_peer", 7, 100).await.unwrap()["exit_code"], 0);
    assert_eq!(
        before,
        f.paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>()
    );
}
