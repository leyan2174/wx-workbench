use super::*;

#[path = "legacy_media_tests.rs"]
mod legacy_media_tests;

#[tokio::test]
async fn q_prepare_voice_returns_only_bounded_internal_payload() {
    let f = Fixture::new(true).await;
    let limits = prepared_audio::Limits {
        max_audio_bytes: 16 * 1024 * 1024,
        max_response_bytes: 24 * 1024 * 1024 - 1024,
    };
    let before = f.snapshot();
    let value = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, limits)
        .await
        .unwrap();
    assert_eq!(value.as_object().unwrap().len(), 1);
    let bytes = serde_json::to_vec(&value["prepared_audio"]).unwrap();
    let voice = prepared_audio::decode(&bytes, limits).unwrap();
    assert_eq!(voice.evidence.message_local_id, 7);
    assert_eq!(voice.evidence.media_local_id, 700);
    assert_eq!(voice, f.message_voice(Some(123)).unwrap());
    let exact = prepared_audio::Limits {
        max_response_bytes: bytes.len(),
        ..limits
    };
    assert!(
        mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, exact)
            .await
            .is_ok()
    );
    assert!(mcp_audio::q_prepare_voice(
        &f.db,
        &f.names,
        "wxid_peer",
        700,
        prepared_audio::Limits {
            max_response_bytes: bytes.len() - 1,
            ..limits
        }
    )
    .await
    .is_err());
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn q_prepare_voice_validates_limits_and_positive_id_before_database_access() {
    let f = Fixture::new(false).await;
    fs::remove_file(f.db.db_dir().join("message/message_0.db")).unwrap();
    let limits = prepared_audio::Limits {
        max_audio_bytes: 1024,
        max_response_bytes: 4096,
    };
    for id in [i64::MIN, -1, 0] {
        let error = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", id, limits)
            .await
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "voice media local_id must be positive"
        );
    }
    let error = mcp_audio::q_prepare_voice(
        &f.db,
        &f.names,
        "wxid_peer",
        700,
        prepared_audio::Limits {
            max_audio_bytes: usize::MAX,
            ..limits
        },
    )
    .await
    .unwrap_err();
    assert_eq!(format!("{error:#}"), "invalid voice preparation limits");
}

#[tokio::test]
async fn q_prepare_voice_ambiguity_and_backend_errors_have_safe_error_chains() {
    let limits = prepared_audio::Limits {
        max_audio_bytes: 1024,
        max_response_bytes: 4096,
    };
    let f = Fixture::new(false).await;
    f.sql(3, "INSERT INTO VoiceInfo VALUES(9,700,999,999,NULL)");
    let error = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, limits)
        .await
        .unwrap_err();
    assert_eq!(format!("{error:#}"), "ambiguous voice media local_id");
    let f = Fixture::new(false).await;
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    f.sql(
        1,
        &format!("INSERT INTO [{table}] VALUES(8,34,123,100,0,NULL)"),
    );
    let error = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, limits)
        .await
        .unwrap_err();
    assert_eq!(format!("{error:#}"), "ambiguous voice message");
    let error = mcp_audio::q_prepare_voice(&f.db, &f.names, "SECRET_UNTRUSTED_CHAT", 700, limits)
        .await
        .unwrap_err();
    assert_eq!(format!("{error:#}"), "voice preparation unavailable");
    fs::write(&f.paths[2], b"SECRET_CORRUPT_DATABASE").unwrap();
    let error = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, limits)
        .await
        .unwrap_err();
    assert_eq!(format!("{error:#}"), "voice preparation unavailable");
}

#[tokio::test]
async fn audio_sql_to_local_ipc_bridge_roundtrips_without_source_changes() {
    let f = Fixture::new(true).await;
    let before = f.snapshot();
    let limits = prepared_audio::Limits {
        max_audio_bytes: 1024,
        max_response_bytes: 4096,
    };
    let value = mcp_audio::q_prepare_voice(&f.db, &f.names, "wxid_peer", 700, limits)
        .await
        .unwrap();
    let payload = serde_json::to_vec(&value["prepared_audio"]).unwrap();
    let decoded = prepared_audio::decode(&payload, limits).unwrap();
    assert_eq!(decoded, f.message_voice(Some(123)).unwrap());
    assert_eq!(before, f.snapshot());
    assert!(mcp_audio::q_prepare_voice(
        &f.db,
        &f.names,
        "wxid_peer",
        700,
        prepared_audio::Limits {
            max_response_bytes: 10,
            ..limits
        }
    )
    .await
    .is_err());
    assert_eq!(before, f.snapshot());
}

struct Fixture {
    _root: tempfile::TempDir,
    db: DbCache,
    names: Names,
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
        let raw = [
            "message/message_0.db",
            "message/message_1.db",
            "message/media_0.db",
            "message/media_1.db",
            "contact/contact.db",
        ];
        let mut keys = HashMap::new();
        let mut mtimes = serde_json::Map::new();
        let mut paths = Vec::new();
        for (index, key) in raw.into_iter().enumerate() {
            let original = source.join(key);
            fs::write(&original, b"synthetic encrypted database").unwrap();
            let raw_key = if upper {
                key.to_ascii_uppercase().replace('/', "\\")
            } else {
                key.into()
            };
            let path = cache.join(format!("{:x}.db", md5::compute(&raw_key)));
            let c = Connection::open(&path).unwrap();
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
            let mt = fs::metadata(&original)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
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
            names: Names {
                map: HashMap::from([("wxid_peer".into(), "peer".into())]),
            },
            paths,
        }
    }
    async fn prepared_voice(&self) -> anyhow::Result<database_media::DatabaseVoice> {
        let limits = prepared_audio::Limits {
            max_audio_bytes: 16 * 1024 * 1024,
            max_response_bytes: 24 * 1024 * 1024 - 1024,
        };
        let value =
            mcp_audio::q_prepare_voice(&self.db, &self.names, "wxid_peer", 700, limits).await?;
        Ok(prepared_audio::decode(
            &serde_json::to_vec(&value["prepared_audio"])?,
            limits,
        )?)
    }
    fn message_voice(
        &self,
        time: Option<i64>,
    ) -> Result<database_media::DatabaseVoice, database_media::DatabaseMediaError> {
        let sources: Vec<_> = [
            "message/message_0.db",
            "message/message_1.db",
            "message/media_0.db",
            "message/media_1.db",
            "contact/contact.db",
        ]
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
        let voice = f.prepared_voice().await.unwrap();
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
        f.prepared_voice().await.unwrap_err().to_string(),
        "ambiguous voice message"
    );
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn audio_missing_unknown_and_corrupt_dependencies_fail_closed() {
    for index in 0..5 {
        let f = Fixture::new(false).await;
        fs::write(&f.paths[index], b"synthetic corrupt database").unwrap();
        let before = f.snapshot();
        assert!(f.prepared_voice().await.is_err(), "corrupt source {index}");
        assert_eq!(before, f.snapshot());
    }
    for key in [
        "message/media_1.db",
        "contact/contact.db",
        "message/message_1.db",
    ] {
        let f = Fixture::new(false).await;
        fs::remove_file(f.db.db_dir().join(key)).unwrap();
        assert!(f.prepared_voice().await.is_err(), "missing {key}");
    }
    for name in ["MEDIA_9.DB", "MESSAGE_9.DB", "MEDIA_BAD.DB"] {
        let f = Fixture::new(false).await;
        fs::write(
            f.db.db_dir().join("message").join(name),
            b"unknown synthetic",
        )
        .unwrap();
        assert!(f.prepared_voice().await.is_err(), "unknown {name}");
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
        let err = f.prepared_voice().await.unwrap_err();
        assert!(!format!("{err:#}").contains("SECRET"));
        assert_eq!(before, f.snapshot());
    }
    let f = Fixture::new(false).await;
    f.sql(
        3,
        "INSERT INTO VoiceInfo VALUES(9,701,123,100,X'232153494C4B5F5633')",
    );
    let before = f.snapshot();
    assert!(f
        .prepared_voice()
        .await
        .unwrap_err()
        .to_string()
        .contains("ambiguous voice media local_id"));
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
        a.prepared_voice().await.unwrap().silk,
        b.prepared_voice().await.unwrap().silk
    );
    assert_eq!(before_a, a.snapshot());
    assert_eq!(before_b, b.snapshot());
}
