use super::*;

#[test]
fn progress_flatten_preserves_legacy_report_and_raw_failure_json() {
    let mut report = BatchReport::default();
    report.progress.record(BatchItemOutcome::Converted);
    report.progress.record(BatchItemOutcome::Existing);
    report.progress.record(BatchItemOutcome::Filtered);
    report.progress.record(BatchItemOutcome::Failed);
    report
        .warnings
        .push("contact database unavailable; using usernames".into());
    report.failures.push(BatchFailure {
        chat_name_id: Some(99),
        local_id: None,
        error: failure_message(BatchFailureStage::Material).into(),
    });
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "total": 4, "success": 2, "failed": 1, "converted": 1,
            "skipped_existing": 1, "filtered": 1,
            "warnings": ["contact database unavailable; using usernames"],
            "failures": [{"chat_name_id": 99, "local_id": null,
                "error": "empty, invalid or oversized voice_data"}]
        })
    );
}

#[test]
fn synthetic_encoder_publishes_then_skips_duplicate_before_material() {
    let mut fixture = Fixture::new();
    let encoder = fixture._temporary.path().join("success.exe");
    fs::copy(
        crate::infrastructure::audio::process_tests::fake_ffmpeg().join("helper.exe"),
        &encoder,
    )
    .unwrap();
    fixture.options.ffmpeg = encoder;
    let data = fs::read(crate::infrastructure::audio::tests::fixtures().join("tone.silk")).unwrap();
    fixture.add(1, 7, Some(&data));
    fixture.add(1, 7, None);
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.converted,
            report.progress.skipped_existing,
            report.progress.failed
        ),
        (2, 1, 1, 0)
    );
    let lane = fixture.options.output_dir.join("same_name/voice");
    let files: Vec<_> = fs::read_dir(&lane)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(fs::read(&files[0]).unwrap(), b"synthetic encoded audio");
    let repeat = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (repeat.progress.converted, repeat.progress.skipped_existing),
        (0, 2)
    );
}

#[test]
fn cancellation_before_batch_creates_no_output() {
    let fixture = Fixture::new();
    fixture.add(1, 1, None);
    assert!(convert_database_checked(&fixture.options, &[], || true).is_err());
    assert!(!fixture.options.output_dir.exists());
}

#[test]
fn cancelled_final_publish_cleans_stage_without_output() {
    let root = tempfile::tempdir().unwrap();
    let staged = tempfile::NamedTempFile::new_in(root.path())
        .unwrap()
        .into_temp_path();
    fs::write(&staged, b"encoded bytes").unwrap();
    let target = root.path().join("out.mp3");
    let mut checks = 0;
    let error = publish_mp3_checked(staged, &target, &[], &mut || {
        checks += 1;
        ensure!(checks < 3, "Audio batch cancelled");
        Ok(())
    })
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!target.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn final_callback_target_race_keeps_winner_and_skips() {
    let root = tempfile::tempdir().unwrap();
    let staged = tempfile::NamedTempFile::new_in(root.path())
        .unwrap()
        .into_temp_path();
    fs::write(&staged, b"our encoded bytes").unwrap();
    let target = root.path().join("out.mp3");
    let mut checks = 0;
    let published = publish_mp3_checked(staged, &target, &[], &mut || {
        checks += 1;
        if checks == 3 {
            fs::write(&target, b"winning output")?;
        }
        Ok(())
    })
    .unwrap();
    assert!(!published);
    assert_eq!(fs::read(&target).unwrap(), b"winning output");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

struct Fixture {
    _temporary: tempfile::TempDir,
    options: BatchOptions,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let decrypted = temporary.path().join("decrypted");
        fs::create_dir_all(decrypted.join("message")).unwrap();
        fs::create_dir_all(decrypted.join("contact")).unwrap();
        let options = BatchOptions {
            media_db: decrypted.join("message/media_0.db"),
            contact_db: decrypted.join("contact/contact.db"),
            output_dir: temporary.path().join("export"),
            contacts: None,
            ffmpeg: PathBuf::from("ffmpeg"),
        };
        let media = Connection::open(&options.media_db).unwrap();
        media.execute_batch("CREATE TABLE Name2Id(user_name TEXT); CREATE TABLE VoiceInfo(chat_name_id INTEGER, create_time INTEGER, local_id INTEGER, voice_data BLOB); INSERT INTO Name2Id(rowid,user_name) VALUES (1,'alice'),(2,'bob'),(3,'charlie');").unwrap();
        let contacts = Connection::open(&options.contact_db).unwrap();
        contacts.execute_batch("CREATE TABLE contact(username TEXT, alias TEXT, remark TEXT, nick_name TEXT); INSERT INTO contact VALUES ('alice','alice_alias','same/name','Alice'),('bob',NULL,'same/name','Bob'),('charlie',NULL,NULL,'Charlie');").unwrap();
        Self {
            _temporary: temporary,
            options,
        }
    }

    fn add(&self, chat: i64, local: i64, data: Option<&[u8]>) {
        Connection::open(&self.options.media_db)
            .unwrap()
            .execute(
                "INSERT INTO VoiceInfo VALUES (?1,1700000000,?2,?3)",
                rusqlite::params![chat, local, data],
            )
            .unwrap();
    }

    fn existing(&self, username: &str, display: &str, local_id: i64) -> PathBuf {
        let contacts = contact_source::read(&self.options.contact_db).unwrap();
        let directory = contact_directory(
            &self.options.output_dir,
            username,
            display,
            contacts.get(username),
        )
        .unwrap();
        let voice = directory.join("voice");
        fs::create_dir_all(&voice).unwrap();
        let date = Local.timestamp_opt(1700000000, 0).single().unwrap();
        let path = voice.join(format!("{}_{local_id}.mp3", date.format("%Y%m%d_%H%M%S")));
        fs::write(&path, b"existing MP3, do not replace").unwrap();
        path
    }
}

#[test]
fn config_resolves_database_and_account_output_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("config.json");
    fs::write(
        &path,
        br#"{"db_dir":"accounts/test_account/db_storage","decrypted_dir":"plain"}"#,
    )
    .unwrap();
    let config = BatchOptions::from_config_file(&path).unwrap();
    let base = temporary.path().canonicalize().unwrap();
    assert_eq!(config.media_db, base.join("plain/message/media_0.db"));
    assert_eq!(config.contact_db, base.join("plain/contact/contact.db"));
    assert_eq!(config.output_dir, base.join("wechat_files/test_account"));
    let options =
        BatchOptions::from_config(&serde_json::json!({"output_base_dir":"chosen"}), &base).unwrap();
    assert_eq!(options.media_db, base.join("decrypted/message/media_0.db"));
    assert_eq!(options.output_dir, base.join("chosen"));
    assert!(BatchOptions::from_config(&serde_json::json!({"output_base_dir":""}), &base).is_err());
}

#[test]
fn filter_matches_legacy_exact_usernames() {
    assert_eq!(parse_contact_filter("  "), None);
    assert_eq!(
        parse_contact_filter(" alice,bob,alice ").unwrap(),
        BTreeSet::from(["alice".into(), "bob".into()])
    );
    assert!(parse_contact_filter("alice, bob").unwrap().contains(" bob"));
}

#[test]
fn windows_names_are_safe_and_bounded() {
    assert_eq!(safe_dirname(" a/b:c*?\"<>|\\ "), "a_b_c_______");
    for name in ["..", ".", "   "] {
        assert_eq!(safe_dirname(name), "unknown");
    }
    assert_eq!(safe_dirname("CON.txt"), "_CON.txt");
    assert_eq!(safe_dirname("lpt1"), "_lpt1");
    assert_eq!(safe_dirname("a\nb. "), "a_b");
    assert!(safe_dirname(&"x".repeat(1000)).len() <= 100);
    for name in [". .".to_owned(), format!("{}x", ".".repeat(100))] {
        assert_eq!(safe_dirname(&name), "unknown");
    }
    assert_eq!(safe_dirname("COM\u{b9}.txt"), "_COM\u{b9}.txt");
    assert_eq!(safe_dirname("CON .txt"), "_CON .txt");
}

#[test]
fn sqlite_filters_skips_existing_and_counts_bad_records() {
    let mut fixture = Fixture::new();
    fixture.add(1, 1, None);
    fixture.add(1, 2, Some(b"invalid silk"));
    fixture.add(2, 3, Some(b"filtered out"));
    fixture.options.contacts = parse_contact_filter("alice");
    let existing = fixture.existing("alice", "same/name", 1);
    let before = fs::read(&fixture.options.media_db).unwrap();
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.success,
            report.progress.failed,
            report.progress.converted,
            report.progress.skipped_existing,
            report.progress.filtered
        ),
        (3, 1, 1, 0, 1, 1)
    );
    assert_eq!(report.failures[0].local_id, Some(2));
    assert_eq!(fs::read(existing).unwrap(), b"existing MP3, do not replace");
    assert_eq!(fs::read(&fixture.options.media_db).unwrap(), before);
    let info = fs::read_to_string(fixture.options.output_dir.join("same_name/.info")).unwrap();
    assert_eq!(
        info,
        "username:  alice\nalias:     alice_alias\nnick_name: Alice\nremark:    same/name\n"
    );
    assert_eq!(
        fs::read_dir(&fixture.options.output_dir).unwrap().count(),
        1
    );
}

#[test]
fn absent_contact_database_falls_back_without_creating_database() {
    let mut fixture = Fixture::new();
    fixture.options.contact_db.set_file_name("missing.db");
    fixture.add(1, 1, Some(b"bad"));
    fixture.add(99, 2, None);
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.failed,
            report.warnings.len()
        ),
        (2, 2, 1)
    );
    assert!(!fixture.options.contact_db.exists());
    assert!(fixture.options.output_dir.join("alice/.info").exists());
    assert!(fixture.options.output_dir.join("unknown_99/.info").exists());
}

#[test]
fn missing_main_database_and_unsafe_output_fail_before_writing() {
    let mut fixture = Fixture::new();
    fixture.options.output_dir = fixture.options.media_db.parent().unwrap().join("voice");
    assert!(convert_database(&fixture.options).is_err());
    assert!(!fixture.options.output_dir.exists());
    fixture.options.output_dir = fixture._temporary.path().join("export");
    fixture.options.media_db.set_file_name("missing.db");
    assert!(convert_database(&fixture.options).is_err());
    assert!(!fixture.options.media_db.exists());
    assert!(!fixture.options.output_dir.exists());
}

#[test]
fn same_display_names_have_separate_info_and_keep_existing_info() {
    let fixture = Fixture::new();
    let contacts = contact_source::read(&fixture.options.contact_db).unwrap();
    let alice = contact_directory(
        &fixture.options.output_dir,
        "alice",
        "same/name",
        contacts.get("alice"),
    )
    .unwrap();
    let bob = contact_directory(
        &fixture.options.output_dir,
        "bob",
        "same/name",
        contacts.get("bob"),
    )
    .unwrap();
    assert_ne!(alice, bob);
    fs::write(
        alice.join(".info"),
        "username:  alice\nalias:     manual edit\n",
    )
    .unwrap();
    assert_eq!(
        contact_directory(
            &fixture.options.output_dir,
            "alice",
            "same/name",
            contacts.get("alice")
        )
        .unwrap(),
        alice
    );
    assert!(fs::read_to_string(alice.join(".info"))
        .unwrap()
        .contains("manual edit"));
    assert!(fs::read_to_string(bob.join(".info"))
        .unwrap()
        .starts_with("username:  bob\n"));
}

#[test]
#[ignore = "requires ffmpeg in PATH; synthetic SQLite end-to-end audio batch"]
fn real_sqlite_batch_converts_and_repeated_run_skips() {
    let fixture = Fixture::new();
    let fixtures = crate::infrastructure::audio::tests::fixtures();
    let silk = fs::read(fixtures.join("multi100.silk")).unwrap();
    fixture.add(1, 1, Some(&silk));
    fixture.add(1, 2, Some(b"bad"));
    fixture.add(2, 3, Some(&silk));
    fixture.add(99, 4, Some(&silk));
    fixture.add(3, 5, None);
    let existing = fixture.existing("charlie", "Charlie", 5);
    let media_before = fs::read(&fixture.options.media_db).unwrap();
    let contacts_before = fs::read(&fixture.options.contact_db).unwrap();
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.success,
            report.progress.failed,
            report.progress.converted,
            report.progress.skipped_existing
        ),
        (5, 4, 1, 3, 1)
    );
    let again = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            again.progress.total,
            again.progress.success,
            again.progress.failed,
            again.progress.converted,
            again.progress.skipped_existing
        ),
        (5, 4, 1, 0, 4)
    );
    assert_eq!(fs::read(existing).unwrap(), b"existing MP3, do not replace");
    assert_eq!(fs::read(&fixture.options.media_db).unwrap(), media_before);
    assert_eq!(
        fs::read(&fixture.options.contact_db).unwrap(),
        contacts_before
    );
    for entry in fs::read_dir(&fixture.options.output_dir).unwrap() {
        let directory = entry.unwrap().path();
        assert!(directory.join(".info").is_file());
        for voice in fs::read_dir(directory.join("voice")).unwrap() {
            let voice = voice.unwrap();
            assert_eq!(voice.path().extension().unwrap(), "mp3");
        }
    }
    println!("batch: 5 SQLite rows, 3 real conversions, 1 existing skip, 1 failure; rerun: 4 skips, 1 failure; databases byte-identical");
}

#[test]
fn missing_encoder_counts_failure_and_removes_temporary_silk() {
    let mut fixture = Fixture::new();
    fixture.options.ffmpeg = fixture._temporary.path().join("missing-ffmpeg.exe");
    let fixtures = crate::infrastructure::audio::tests::fixtures();
    fixture.add(1, 1, Some(&fs::read(fixtures.join("tone.silk")).unwrap()));
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.success,
            report.progress.failed
        ),
        (1, 0, 1)
    );
    assert!(report.failures[0].error.contains("start ffmpeg"));
    let voice = fixture.options.output_dir.join("same_name/voice");
    assert_eq!(fs::read_dir(voice).unwrap().count(), 0);
}

#[test]
fn malformed_sqlite_rows_are_individual_failures() {
    let fixture = Fixture::new();
    Connection::open(&fixture.options.media_db).unwrap().execute_batch(
        "INSERT INTO VoiceInfo VALUES (1,9223372036854775807,1,NULL); INSERT INTO VoiceInfo VALUES (1,1700000000,NULL,NULL); INSERT INTO VoiceInfo VALUES (NULL,1700000000,3,NULL);"
    ).unwrap();
    let report = convert_database(&fixture.options).unwrap();
    assert_eq!(
        (
            report.progress.total,
            report.progress.failed,
            report.failures.len()
        ),
        (3, 3, 3)
    );
    assert_eq!(report.progress.success, 0);
}

#[test]
fn late_existing_mp3_is_not_overwritten_and_staging_is_removed() {
    let temporary = tempfile::tempdir().unwrap();
    let mut staged = tempfile::NamedTempFile::new_in(temporary.path()).unwrap();
    staged.write_all(b"new encoded MP3").unwrap();
    let staged_path = staged.path().to_owned();
    let target = temporary.path().join("target.mp3");
    fs::write(&target, b"published by another exporter").unwrap();
    assert!(!publish_mp3(staged.into_temp_path(), &target).unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"published by another exporter");
    assert!(!staged_path.exists());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
}

#[test]
fn failed_batch_publication_preserves_directory_and_cleans_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let staged = tempfile::NamedTempFile::new_in(temporary.path())
        .unwrap()
        .into_temp_path();
    let staged_path = staged.to_path_buf();
    let target = temporary.path().join("target.mp3");
    fs::create_dir(&target).unwrap();
    assert!(publish_mp3(staged, &target).is_err());
    assert!(target.is_dir());
    assert!(!staged_path.exists());
}
