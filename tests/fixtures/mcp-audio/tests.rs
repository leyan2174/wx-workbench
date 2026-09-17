use super::*;

#[path = "legacy_media_tests.rs"]
mod legacy_media_tests;

#[path = "../encrypted_cache.rs"]
mod encrypted_cache;

const SOURCES: [&str; 5] = [
    "message/message_0.db",
    "message/message_1.db",
    "message/media_0.db",
    "message/media_1.db",
    "contact/contact.db",
];

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    paths: Vec<PathBuf>,
}

impl Fixture {
    async fn new(upper: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("wxid_test/db_storage");
        let cache = root.path().join("cache");
        fs::create_dir_all(source.join("message")).unwrap();
        fs::create_dir(source.join("contact")).unwrap();
        fs::create_dir(&cache).unwrap();
        let mut keys = HashMap::new();
        let mut mtimes = serde_json::Map::new();
        let mut paths = Vec::new();
        for (index, key) in SOURCES.into_iter().enumerate() {
            let original = source.join(key);
            let raw_key = if upper {
                key.to_ascii_uppercase().replace('/', "\\")
            } else {
                key.into()
            };
            let path = cache.join(format!("{:x}.db", md5::compute(&raw_key)));
            let c = encrypted_cache::sqlite(&path);
            if index < 2 {
                let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
                c.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER,WCDB_CT_message_content INTEGER,message_content BLOB)")).unwrap();
                if index == 0 {
                    c.execute_batch(&format!(
                        "INSERT INTO [{table}] VALUES(7,34,123,100,0,NULL)"
                    ))
                    .unwrap();
                }
            } else if index < 4 {
                c.execute_batch("CREATE TABLE Name2Id(user_name TEXT); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB); INSERT INTO Name2Id(rowid,user_name) VALUES(9,'wxid_peer')").unwrap();
                if index == 2 {
                    c.execute(
                        "INSERT INTO VoiceInfo VALUES(9,700,123,100,?1)",
                        [b"\x02#!SILK_V3synthetic".as_slice()],
                    )
                    .unwrap();
                }
            } else {
                c.execute_batch("CREATE TABLE contact(username TEXT)")
                    .unwrap();
            }
            drop(c);
            let mt = encrypted_cache::seed(&path, &original);
            mtimes.insert(raw_key.clone(), json!({"db_mt":mt,"wal_mt":0,"path":path}));
            keys.insert(raw_key, "11".repeat(32));
            paths.push(path);
        }
        let file = cache.join("_mtimes.json");
        fs::write(&file, serde_json::to_vec(&mtimes).unwrap()).unwrap();
        let db = DbCache::with_dirs(source, cache, file, keys).await.unwrap();
        Self {
            _root: root,
            db,
            paths,
        }
    }
    fn sources(&self) -> Vec<database_media::DecryptedSource> {
        SOURCES
            .into_iter()
            .zip(&self.paths)
            .map(|(source, path)| database_media::DecryptedSource {
                source: source.into(),
                path: path.clone(),
            })
            .collect()
    }

    async fn raw_voice(&self) -> anyhow::Result<database_media::DatabaseVoice> {
        database_media::resolve_voice_media_row(
            &self.sources(),
            "wxid_peer",
            700,
            "message/media_0.db",
            1,
        )
        .map_err(Into::into)
    }
    fn message_voice(
        &self,
        time: Option<i64>,
    ) -> Result<database_media::DatabaseVoice, database_media::DatabaseMediaError> {
        let sources: Vec<_> = SOURCES
            .into_iter()
            .zip(&self.paths)
            .map(|(source, path)| database_media::DecryptedSource {
                source: source.into(),
                path: path.clone(),
            })
            .collect();
        database_media::resolve_voice_sources(
            &sources,
            database_media::MessageIdentity {
                username: "wxid_peer",
                source: "message/message_0.db",
                local_id: 7,
            },
            time,
        )
    }
    fn snapshot(&self) -> Vec<Vec<u8>> {
        self.paths
            .iter()
            .map(|p| fs::read(p).unwrap())
            .chain(
                database_media::source_files(self.db.db_dir())
                    .unwrap()
                    .iter()
                    .map(|p| fs::read(p).unwrap()),
            )
            .collect()
    }
    fn sql(&self, index: usize, sql: &str) {
        Connection::open(&self.paths[index])
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}

#[tokio::test]
async fn audio_real_cache_raw_keys_exact_bytes_and_evidence_are_read_only() {
    for upper in [false, true] {
        let f = Fixture::new(upper).await;
        assert_eq!(f.db.raw_db_keys().len(), 5);
        assert_eq!(f.db.media_db_keys().len(), 2);
        assert!(f
            .db
            .raw_db_keys()
            .iter()
            .all(|k| !k.contains(&"11".repeat(32))));
        let before = f.snapshot();
        let voice = f.raw_voice().await.unwrap();
        assert_eq!(voice.silk, b"\x02#!SILK_V3synthetic");
        assert_eq!(voice.evidence.username, "wxid_peer");
        assert_eq!(voice.evidence.message_source, "message/message_0.db");
        assert_eq!(voice.evidence.message_local_id, 7);
        assert_eq!(voice.evidence.create_time, 123);
        assert_eq!(voice.evidence.media_local_id, 700);
        assert_eq!(voice.evidence.media_source, "message/media_0.db");
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn audio_ambiguity_precedes_type_and_time_can_disambiguate() {
    let f = Fixture::new(false).await;
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    f.sql(
        0,
        &format!("INSERT INTO [{table}] VALUES(7,1,124,101,0,NULL)"),
    );
    let before = f.snapshot();
    assert_eq!(
        f.message_voice(None).unwrap_err().kind,
        database_media::ErrorKind::AmbiguousMessage
    );
    assert_eq!(
        f.message_voice(Some(123)).unwrap().evidence.create_time,
        123
    );
    assert_eq!(
        f.message_voice(Some(124)).unwrap_err().kind,
        database_media::ErrorKind::NotVoice
    );
    assert_eq!(before, f.snapshot());
    f.sql(
        1,
        &format!("INSERT INTO [{table}] VALUES(7,34,123,100,0,NULL)"),
    );
    let before = f.snapshot();
    assert_eq!(
        f.raw_voice()
            .await
            .unwrap_err()
            .downcast_ref::<database_media::DatabaseMediaError>()
            .unwrap()
            .kind,
        database_media::ErrorKind::AmbiguousMessage
    );
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn raw_voice_rejects_corrupt_or_missing_decrypted_sources() {
    for (index, source) in SOURCES.iter().enumerate() {
        let f = Fixture::new(false).await;
        fs::write(&f.paths[index], b"synthetic corrupt database").unwrap();
        let before = f.snapshot();
        assert!(f.raw_voice().await.is_err(), "corrupt source {}", source);
        assert_eq!(before, f.snapshot());
        fs::remove_file(&f.paths[index]).unwrap();
        assert!(f.raw_voice().await.is_err(), "missing source {}", source);
    }
}

#[tokio::test]
async fn audio_join_conflicts_invalid_blob_and_cross_shard_matches_are_rejected() {
    for sql in [
        "UPDATE VoiceInfo SET create_time=124",
        "UPDATE VoiceInfo SET voice_data=NULL",
        "UPDATE VoiceInfo SET voice_data='SECRET-not-blob'",
        "UPDATE VoiceInfo SET voice_data=X'0001'",
        "UPDATE VoiceInfo SET voice_data=zeroblob(16777217)",
        "INSERT INTO Name2Id(user_name) VALUES('wxid_peer')",
        "INSERT INTO VoiceInfo SELECT * FROM VoiceInfo",
        "UPDATE VoiceInfo SET svr_id=101",
    ] {
        let f = Fixture::new(false).await;
        f.sql(2, sql);
        let before = f.snapshot();
        let err = f.raw_voice().await.unwrap_err();
        assert!(!format!("{err:#}").contains("SECRET"));
        assert_eq!(before, f.snapshot());
    }
    let f = Fixture::new(false).await;
    f.sql(
        3,
        "INSERT INTO VoiceInfo VALUES(9,701,123,100,X'232153494C4B5F5633')",
    );
    let before = f.snapshot();
    assert_eq!(
        f.raw_voice()
            .await
            .unwrap_err()
            .downcast_ref::<database_media::DatabaseMediaError>()
            .unwrap()
            .kind,
        database_media::ErrorKind::AmbiguousMedia
    );
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn audio_explicit_accounts_do_not_share_media_or_outputs() {
    let a = Fixture::new(false).await;
    let b = Fixture::new(false).await;
    b.sql(2, "UPDATE VoiceInfo SET voice_data=X'232153494C4B5F563342'");
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    assert_ne!(
        a.raw_voice().await.unwrap().silk,
        b.raw_voice().await.unwrap().silk
    );
    assert_eq!(before_a, a.snapshot());
    assert_eq!(before_b, b.snapshot());
}
