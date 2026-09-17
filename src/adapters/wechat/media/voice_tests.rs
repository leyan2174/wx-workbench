use super::*;

#[test]
fn raw_coordinate_reverse_join_preserves_bytes_and_unknowns() {
    let f = Fixture::valid();
    let sources = explicit_sources(&f);
    let raw = resolve_voice_media_row(&sources, "alice", 700, "message/media_0.db", 1).unwrap();
    assert_eq!(raw.silk, b"\x02#!SILK_V3\0\x01synthetic");
    assert_eq!(raw.evidence.message_local_id, 7);
    assert_eq!(raw.evidence.media_rowid, 1);
    assert_eq!(raw.sender, None);
    assert_eq!(raw.duration_ms, None);
    assert!(resolve_voice_media_row(&sources, "alice", 7, "message/media_0.db", 1).is_err());
    assert!(resolve_voice_media_row(&sources, "alice", 700, "../media_0.db", 1).is_err());
    f.message("message_1.db", "alice", 99, 100, 123, 34);
    assert_eq!(
        resolve_voice_media_row(&explicit_sources(&f), "alice", 700, "message/media_0.db", 1)
            .unwrap_err()
            .kind,
        ErrorKind::AmbiguousMessage
    );
}

#[test]
fn raw_container_and_duration_do_not_repair_or_guess() {
    assert!(is_raw_silk(b"\x02#!SILK_V3\0\xff"));
    assert!(is_raw_silk(b"#!SILK_V3\0"));
    assert!(!is_raw_silk(b"\x02\x02#!SILK_V3"));
    assert_eq!(
        voice_duration_ms("<msg><voicemsg voicelength='0'/></msg>"),
        Some(0)
    );
    assert_eq!(
        voice_duration_ms("<msg><voicemsg voicelength='1234'/></msg>"),
        Some(1234)
    );
    for xml in [
        "",
        "<msg/>",
        "<msg><voicemsg voicelength='-1'/></msg>",
        "<msg><voicemsg voicelength='1'/><voicemsg voicelength='2'/></msg>",
    ] {
        assert_eq!(voice_duration_ms(xml), None);
    }
}

fn explicit_sources(f: &Fixture) -> Vec<DecryptedSource> {
    source_files(f.root())
        .unwrap()
        .into_iter()
        .map(|path| DecryptedSource {
            source: path
                .strip_prefix(f.root().canonicalize().unwrap())
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            path,
        })
        .collect()
}

#[test]
fn explicit_paths_share_legacy_join_and_filter_exact_time_including_zero() {
    let f = Fixture::valid();
    let id = MessageIdentity {
        username: "alice",
        source: "MESSAGE\\MESSAGE_0.DB",
        local_id: 7,
    };
    let sources = explicit_sources(&f);
    let before = f.snapshot();
    assert_eq!(
        resolve_voice_sources(&sources, id, None).unwrap(),
        f.get("alice", "message/message_0.db", 7).unwrap()
    );
    assert_eq!(
        resolve_voice_sources(&sources, id, Some(0))
            .unwrap_err()
            .kind,
        ErrorKind::MessageNotFound
    );
    assert_eq!(before, f.snapshot());
    f.message("message_0.db", "alice", 7, 101, 0, 34);
    f.media("media_0.db", "alice", 9, 701, 101, 0, b"#!SILK_V3zero");
    let before = f.snapshot();
    assert_eq!(
        resolve_voice_sources(&sources, id, None).unwrap_err().kind,
        ErrorKind::AmbiguousMessage
    );
    assert_eq!(
        resolve_voice_sources(&sources, id, Some(0)).unwrap().silk,
        b"#!SILK_V3zero"
    );
    assert_eq!(
        resolve_voice_sources(&sources, id, Some(123))
            .unwrap()
            .evidence
            .server_id,
        100
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn explicit_sources_reject_alias_missing_duplicate_and_active_files() {
    let f = Fixture::valid();
    let id = MessageIdentity {
        username: "alice",
        source: "message/message_0.db",
        local_id: 7,
    };
    let mut sources = explicit_sources(&f);
    let before = f.snapshot();
    sources.push(sources[0].clone());
    assert_eq!(
        resolve_voice_sources(&sources, id, None).unwrap_err().kind,
        ErrorKind::InvalidIdentity
    );
    sources.last_mut().unwrap().source = "message/message_9.db".into();
    assert_eq!(
        resolve_voice_sources(&sources, id, None).unwrap_err().kind,
        ErrorKind::UnsafePath
    );
    sources.last_mut().unwrap().path = f.path("missing.db");
    assert!(resolve_voice_sources(&sources, id, None).is_err());
    sources.pop();
    assert_eq!(before, f.snapshot());
    fs::write(f.path("media_0.db-wal"), b"synthetic active sidecar").unwrap();
    let before = f.snapshot();
    assert_eq!(
        resolve_voice_sources(&sources, id, None).unwrap_err().kind,
        ErrorKind::ActiveDatabase
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn explicit_sources_check_unselected_message_and_contact_databases() {
    for key in ["message/message_1.db", "contact/contact.db"] {
        let f = Fixture::valid();
        let mut sources = explicit_sources(&f);
        let path = f.root().join("unrelated-hash.db");
        fs::write(&path, b"synthetic corrupt database").unwrap();
        sources.push(DecryptedSource {
            source: key.into(),
            path,
        });
        let before = f.snapshot();
        let err = resolve_voice_sources(
            &sources,
            MessageIdentity {
                username: "alice",
                source: "message/message_0.db",
                local_id: 7,
            },
            None,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::DatabaseRead);
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn explicit_media_schema_errors_cannot_hide_behind_earlier_ambiguity() {
    let f = Fixture::valid();
    f.media(
        "media_1.db",
        "alice",
        9,
        701,
        100,
        123,
        b"#!SILK_V3duplicate",
    );
    Connection::open(f.path("media_2.db"))
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(value)")
        .unwrap();
    let sources = explicit_sources(&f);
    let before = f.snapshot();
    assert_eq!(
        resolve_voice_sources(
            &sources,
            MessageIdentity {
                username: "alice",
                source: "message/message_0.db",
                local_id: 7
            },
            None
        )
        .unwrap_err()
        .kind,
        ErrorKind::UnsupportedSchema
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn generated_rowid_aliases_are_rejected_in_every_evidence_table() {
    let message_table = format!("Msg_{:x}", md5::compute("alice"));
    for (file, table) in [
        ("message_0.db", message_table.as_str()),
        ("media_0.db", "Name2Id"),
        ("media_0.db", "VoiceInfo"),
    ] {
        for alias in ["rowid", "_rowid_", "oid"] {
            let f = Fixture::valid();
            Connection::open(f.path(file))
                .unwrap()
                .execute_batch(&format!(
                "ALTER TABLE [{table}] ADD COLUMN [{alias}] INTEGER GENERATED ALWAYS AS (9) VIRTUAL"
            ))
                .unwrap();
            let before = f.snapshot();
            let err = f.get("alice", "message/message_0.db", 7).unwrap_err();
            assert_eq!(err.kind, ErrorKind::UnsupportedSchema, "{table}/{alias}");
            assert_eq!(err.stage, "shadowed SQLite rowid");
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn complete_column_check_rejects_stored_aliases_but_allows_ordinary_rowid_tables() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE Ordinary(id INTEGER PRIMARY KEY, user_name TEXT, extra INTEGER GENERATED ALWAYS AS (9) STORED)").unwrap();
    require_table(&c, "Ordinary", &["user_name"]).unwrap();
    for alias in ["rowid", "_rowid_", "oid"] {
        c.execute_batch(&format!("CREATE TABLE Malformed(user_name TEXT, [{alias}] INTEGER GENERATED ALWAYS AS (9) STORED)")).unwrap();
        assert_eq!(
            require_table(&c, "Malformed", &["user_name"])
                .unwrap_err()
                .kind,
            ErrorKind::UnsupportedSchema
        );
        c.execute_batch("DROP TABLE Malformed").unwrap();
    }
}

#[test]
fn views_and_without_rowid_are_rejected_in_every_evidence_table() {
    let message_table = format!("Msg_{:x}", md5::compute("alice"));
    for (file, table, columns) in [
        ("message_0.db", message_table.as_str(), "local_id INTEGER PRIMARY KEY,local_type INTEGER,create_time INTEGER,server_id INTEGER"),
        ("media_0.db", "Name2Id", "user_name TEXT PRIMARY KEY"),
        ("media_0.db", "VoiceInfo", "local_id INTEGER PRIMARY KEY,chat_name_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB"),
    ] {
        for view in [false, true] {
            let f = Fixture::valid();
            let c = Connection::open(f.path(file)).unwrap();
            if view {
                c.execute_batch(&format!("ALTER TABLE [{table}] RENAME TO Original; CREATE VIEW [{table}] AS SELECT * FROM Original")).unwrap();
            } else {
                c.execute_batch(&format!("DROP TABLE [{table}]; CREATE TABLE [{table}]({columns}) WITHOUT ROWID")).unwrap();
            }
            drop(c);
            let before = f.snapshot();
            assert_eq!(f.get("alice", "message/message_0.db", 7).unwrap_err().kind, ErrorKind::UnsupportedSchema, "{table}/view={view}");
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn uppercase_media_preserves_disk_name_and_canonical_evidence() {
    let f = Fixture::new();
    f.message("message_0.db", "alice", 7, 100, 123, 34);
    f.media("MEDIA_1.DB", "alice", 9, 700, 100, 123, b"#!SILK_V3bytes");
    let before = f.snapshot();
    assert_eq!(
        media_shards(&f.root().join("message")).unwrap(),
        ["MEDIA_1.DB"]
    );
    let voice = f.get("alice", "message/message_0.db", 7).unwrap();
    assert_eq!(voice.evidence.media_source, "message/media_1.db");
    assert_eq!(voice.silk, b"#!SILK_V3bytes");
    assert_eq!(before, f.snapshot());
}

#[test]
fn uppercase_media_does_not_hide_cross_shard_ambiguity() {
    let f = Fixture::valid();
    f.media(
        "MEDIA_1.DB",
        "alice",
        19,
        701,
        100,
        123,
        b"#!SILK_V3duplicate",
    );
    let before = f.snapshot();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::AmbiguousMedia
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn malformed_uppercase_media_candidate_is_not_silently_skipped() {
    let f = Fixture::valid();
    fs::write(f.path("MEDIA_BAD.DB"), b"synthetic malformed candidate").unwrap();
    let before = f.snapshot();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::UnsupportedSchema
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn media_hard_link_aliases_are_rejected_even_without_matching_contact() {
    let f = Fixture::valid();
    f.media("MEDIA_1.DB", "bob", 19, 701, 200, 123, b"#!SILK_V3bob");
    fs::hard_link(f.path("MEDIA_1.DB"), f.path("media_2.db")).unwrap();
    let before = f.snapshot();
    let error = f.get("alice", "message/message_0.db", 7).unwrap_err();
    assert_eq!(error.kind, ErrorKind::AmbiguousMedia);
    assert_eq!(error.stage, "media sources alias the same file");
    assert_eq!(before, f.snapshot());
}

#[test]
fn shadowed_rowid_is_not_an_identity_proof() {
    let f = Fixture::valid();
    Connection::open(f.path("media_0.db"))
        .unwrap()
        .execute_batch("ALTER TABLE VoiceInfo ADD COLUMN rowid INTEGER DEFAULT 1")
        .unwrap();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::UnsupportedSchema
    );
}

#[test]
fn immutable_uri_handles_unicode_spaces_percent_and_hash() {
    let f = Fixture {
        dir: tempfile::Builder::new()
            .prefix("wx 合成 #% ")
            .tempdir()
            .unwrap(),
    };
    fs::create_dir(f.root().join("message")).unwrap();
    f.message("message_0.db", "alice", 7, 100, 123, 34);
    f.media("media_0.db", "alice", 9, 700, 100, 123, b"#!SILK_V3bytes");
    let before = f.snapshot();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap().silk,
        b"#!SILK_V3bytes"
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn corrupt_database_is_reported_and_missing_media_is_not_created() {
    let f = Fixture::valid();
    fs::write(f.path("media_0.db"), b"synthetic corrupt database").unwrap();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::DatabaseRead
    );
    let empty = Fixture::new();
    empty.message("message_0.db", "alice", 7, 100, 123, 34);
    let before = empty.snapshot();
    assert_eq!(
        empty
            .get("alice", "message/message_0.db", 7)
            .unwrap_err()
            .kind,
        ErrorKind::MissingDatabase
    );
    assert_eq!(before, empty.snapshot());
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("message")).unwrap();
        Self { dir }
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn path(&self, file: &str) -> PathBuf {
        self.root().join("message").join(file)
    }
    fn message(&self, file: &str, username: &str, id: i64, server: i64, time: i64, kind: i64) {
        let conn = Connection::open(self.path(file)).unwrap();
        let table = format!("Msg_{:x}", md5::compute(username));
        conn.execute_batch(&format!("CREATE TABLE IF NOT EXISTS [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER)")).unwrap();
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES (?1,?2,?3,?4)"),
            [id, kind, time, server],
        )
        .unwrap();
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "测试辅助函数逐列构造两张关联表，保留原始数据库字段以检验身份冲突"
    )]
    fn media(
        &self,
        file: &str,
        username: &str,
        chat_id: i64,
        local_id: i64,
        server: i64,
        time: i64,
        data: &[u8],
    ) {
        let conn = Connection::open(self.path(file)).unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS Name2Id(user_name TEXT COLLATE NOCASE); CREATE TABLE IF NOT EXISTS VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB)").unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO Name2Id(rowid,user_name) VALUES (?1,?2)",
            rusqlite::params![chat_id, username],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO VoiceInfo VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![chat_id, local_id, time, server, data],
        )
        .unwrap();
    }
    fn valid() -> Self {
        let f = Self::new();
        f.message("message_0.db", "alice", 7, 100, 123, 34);
        f.media(
            "media_0.db",
            "alice",
            9,
            700,
            100,
            123,
            b"\x02#!SILK_V3\0\x01synthetic",
        );
        f
    }
    fn get(&self, username: &str, source: &str, local_id: i64) -> Result<DatabaseVoice> {
        resolve_voice(
            self.root(),
            MessageIdentity {
                username,
                source,
                local_id,
            },
        )
    }
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries: Vec<_> = fs::read_dir(self.root().join("message"))
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        entries.sort();
        entries
    }
}

#[test]
fn exact_bytes_evidence_and_read_only() {
    let f = Fixture::valid();
    let before = f.snapshot();
    let voice = f.get("alice", "message/message_0.db", 7).unwrap();
    assert_eq!(voice.silk, b"\x02#!SILK_V3\0\x01synthetic");
    assert_eq!(voice.evidence.message_local_id, 7);
    assert_eq!(voice.evidence.media_local_id, 700);
    assert_eq!(voice.evidence.server_id, 100);
    assert_eq!(voice.evidence.media_chat_name_id, 9);
    assert_eq!(voice.evidence.media_source, "message/media_0.db");
    assert_eq!(before, f.snapshot());
    assert_eq!(f.get("alice", r"message\message_0.db", 7).unwrap(), voice);
}

#[test]
fn same_local_id_across_message_shards_uses_selected_server_id() {
    let f = Fixture::valid();
    f.message("message_1.db", "alice", 7, 200, 456, 34);
    // 两组媒体 local_id 相同，媒体分片编号也不与消息分片编号对应。
    f.media("media_8.db", "alice", 3, 700, 200, 456, b"#!SILK_V3second");
    assert_eq!(
        f.get("alice", "message/message_0.db", 7)
            .unwrap()
            .evidence
            .server_id,
        100
    );
    let second = f.get("alice", "message/message_1.db", 7).unwrap();
    assert_eq!(second.silk, b"#!SILK_V3second");
    assert_eq!(second.evidence.media_source, "message/media_8.db");
    assert_eq!(second.evidence.server_id, 200);
}

#[test]
fn same_ids_across_contacts_and_nocase_collation_do_not_leak() {
    let f = Fixture::valid();
    f.message("message_0.db", "bob", 7, 100, 123, 34);
    f.media("media_0.db", "bob", 10, 700, 100, 123, b"#!SILK_V3bob");
    f.message("message_0.db", "ALICE", 7, 100, 123, 34);
    assert_eq!(
        f.get("bob", "message/message_0.db", 7).unwrap().silk,
        b"#!SILK_V3bob"
    );
    assert_eq!(
        f.get("ALICE", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::MediaNotFound
    );
    assert!(f
        .get("alice", "message/message_0.db", 7)
        .unwrap()
        .silk
        .ends_with(b"synthetic"));
}

#[test]
fn roots_are_separate_accounts_even_for_identical_identity() {
    let a = Fixture::valid();
    let b = Fixture::new();
    b.message("message_0.db", "alice", 7, 100, 123, 34);
    b.media(
        "media_0.db",
        "alice",
        1,
        700,
        100,
        123,
        b"#!SILK_V3account-b",
    );
    assert_ne!(
        a.get("alice", "message/message_0.db", 7).unwrap().silk,
        b.get("alice", "message/message_0.db", 7).unwrap().silk
    );
}

#[test]
fn duplicate_message_is_rejected_before_type_filter() {
    let f = Fixture::valid();
    f.message("message_0.db", "alice", 7, 999, 123, 1);
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::AmbiguousMessage
    );
}

#[test]
fn duplicate_media_rows_or_shards_are_never_first_match_wins() {
    for duplicate_shard in [false, true] {
        let f = Fixture::valid();
        f.media(
            if duplicate_shard {
                "media_9.db"
            } else {
                "media_0.db"
            },
            "alice",
            9,
            700,
            100,
            123,
            b"#!SILK_V3same",
        );
        assert_eq!(
            f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
            ErrorKind::AmbiguousMedia
        );
    }
}

#[test]
fn duplicate_username_mapping_is_rejected() {
    let f = Fixture::valid();
    Connection::open(f.path("media_0.db"))
        .unwrap()
        .execute("INSERT INTO Name2Id(user_name) VALUES ('alice')", [])
        .unwrap();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::AmbiguousContact
    );
}

#[test]
fn missing_message_media_and_source_do_not_fallback() {
    let f = Fixture::valid();
    assert_eq!(
        f.get("alice", "message/message_0.db", 999)
            .unwrap_err()
            .kind,
        ErrorKind::MessageNotFound
    );
    assert_eq!(
        f.get("alice", "message/message_77.db", 7).unwrap_err().kind,
        ErrorKind::MissingDatabase
    );
    f.message("message_1.db", "alice", 7, 888, 123, 34);
    assert_eq!(
        f.get("alice", "message/message_1.db", 7).unwrap_err().kind,
        ErrorKind::MediaNotFound
    );
    assert!(!f.path("message_77.db").exists());
}

#[test]
fn timestamp_conflict_and_zero_server_id_reject_local_id_guessing() {
    let f = Fixture::valid();
    f.message("message_1.db", "alice", 7, 100, 999, 34);
    f.message("message_2.db", "alice", 7, 0, 123, 34);
    for shard in [1, 2] {
        assert_eq!(
            f.get("alice", &format!("message/message_{shard}.db"), 7)
                .unwrap_err()
                .kind,
            ErrorKind::ConflictingEvidence
        );
    }
}

#[test]
fn missing_server_evidence_is_unsupported_even_with_matching_local_id() {
    let f = Fixture::valid();
    let table = format!("Msg_{:x}", md5::compute("alice"));
    Connection::open(f.path("message_2.db")).unwrap().execute_batch(&format!(
        "CREATE TABLE [{table}](local_id,local_type,create_time); INSERT INTO [{table}] VALUES(7,34,123)"
    )).unwrap();
    assert_eq!(
        f.get("alice", "message/message_2.db", 7).unwrap_err().kind,
        ErrorKind::UnsupportedSchema
    );
    Connection::open(f.path("media_0.db"))
        .unwrap()
        .execute_batch("ALTER TABLE VoiceInfo DROP COLUMN svr_id")
        .unwrap();
    assert_eq!(
        f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
        ErrorKind::UnsupportedSchema
    );
}

#[test]
fn source_path_injection_traversal_bare_name_and_relative_root_rejected() {
    let f = Fixture::valid();
    for source in [
        "message_0.db",
        "../message/message_0.db",
        "message/../message_0.db",
        "message/message_0.db?mode=rw",
        "C:/other/message_0.db",
        "message/media_0.db",
        "message/message_0.db:stream",
    ] {
        assert_eq!(
            f.get("alice", source, 7).unwrap_err().kind,
            ErrorKind::InvalidIdentity,
            "{source}"
        );
    }
    assert_eq!(
        resolve_voice(
            Path::new("relative"),
            MessageIdentity {
                username: "alice",
                source: "message/message_0.db",
                local_id: 7
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::UnsafePath
    );
}

#[test]
fn sidecars_cannot_be_silently_ignored_by_immutable_mode() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let f = Fixture::valid();
        fs::write(f.path(&format!("media_0.db{suffix}")), b"synthetic sidecar").unwrap();
        let before = f.snapshot();
        assert_eq!(
            f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
            ErrorKind::ActiveDatabase
        );
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn non_voice_and_high_type_flags() {
    let f = Fixture::valid();
    f.message("message_1.db", "alice", 7, 100, 123, 1);
    assert_eq!(
        f.get("alice", "message/message_1.db", 7).unwrap_err().kind,
        ErrorKind::NotVoice
    );
    f.message("message_2.db", "alice", 7, 100, 123, (1_i64 << 32) | 34);
    assert!(f.get("alice", "message/message_2.db", 7).is_ok());
}

#[test]
fn empty_non_silk_non_blob_and_oversized_data_fail_without_outputs() {
    for value in [
        "NULL",
        "X''",
        "X'010203'",
        "'#!SILK_V3text'",
        "zeroblob(16777217)",
    ] {
        let f = Fixture::valid();
        Connection::open(f.path("media_0.db"))
            .unwrap()
            .execute_batch(&format!("UPDATE VoiceInfo SET voice_data={value}"))
            .unwrap();
        let before = f.snapshot();
        assert_eq!(
            f.get("alice", "message/message_0.db", 7).unwrap_err().kind,
            ErrorKind::InvalidVoiceData
        );
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn quoted_username_is_bound_not_interpolated() {
    let f = Fixture::new();
    let username = "x' OR 1=1--";
    f.message("message_0.db", username, 1, 444, 123, 34);
    f.media("media_0.db", username, 1, 8, 444, 123, b"#!SILK_V3quoted");
    assert_eq!(
        f.get(username, "message/message_0.db", 1).unwrap().silk,
        b"#!SILK_V3quoted"
    );
}
