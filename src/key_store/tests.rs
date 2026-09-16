use super::*;
use std::fs;

#[test]
fn secret_record_rejects_audio_parameters_without_discarding_database_material() {
    let record = serde_json::json!({
        "version": 1, "account": "synthetic", "revision": 1,
        "account_key": null, "image_key": null,
        "database_keys": {"message/media_0.db": {
            "bytes": vec![0x31u8; 32], "verification": "verified"
        }}
    });
    let parsed: Record = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(
        parsed.database_keys["message/media_0.db"].bytes,
        vec![0x31; 32]
    );
    for field in ["voice_key", "audio_key", "codec", "sample_rate", "channels"] {
        let mut invalid = record.clone();
        invalid[field] = serde_json::json!(24_000);
        assert!(
            serde_json::from_value::<Record>(invalid).is_err(),
            "{field}"
        );
    }
}

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("db_storage");
        fs::create_dir(&db).unwrap();
        let anchor = root.path().join("all_keys.json");
        let store = Store::new(
            &db,
            &anchor,
            &root.path().join("keys.dpapi"),
            vec![db.clone(), anchor.clone()],
        )
        .unwrap();
        Self { root, store }
    }
}

#[test]
fn real_dpapi_roundtrip_preserves_all_materials_and_private_acl() {
    let fixture = Fixture::new();
    let account = [0x17; 32];
    let aes = *b"syntheticAESkey1";
    let databases = HashMap::from([("contact/contact.db".into(), "31".repeat(32))]);
    let one = fixture
        .store
        .update(
            Some(0),
            &[Update::Account(&account, Verification::Unverified)],
        )
        .unwrap();
    assert_eq!(one.revision(), 1);
    let two = fixture
        .store
        .update(
            Some(1),
            &[Update::Databases(&databases, Verification::Verified)],
        )
        .unwrap();
    assert_eq!(two.account_key().unwrap(), &account);
    assert!(two.verified_account_key().is_none());
    let three = fixture
        .store
        .update(
            Some(2),
            &[Update::Image(&aes, 0x51, Verification::Verified)],
        )
        .unwrap();
    assert_eq!(three.revision(), 3);
    assert_eq!(three.account_key().unwrap(), &account);
    assert_eq!(three.database_keys(), databases);
    assert_eq!(three.image_key(), Some((aes, 0x51)));
    let ciphertext = fs::read(fixture.store.path()).unwrap();
    assert!(!ciphertext.windows(32).any(|window| window == account));
    assert!(!ciphertext.windows(aes.len()).any(|window| window == aes));
    assert!(!format!("{three:?}").contains("synthetic"));
    crate::private_file::assert_private_acl(fixture.store.path());
}

#[test]
fn account_and_derived_databases_publish_in_one_revision_or_not_at_all() {
    let fixture = Fixture::new();
    let databases = HashMap::from([("message/message_0.db".into(), "31".repeat(32))]);
    let account = [0x17; 32];
    let saved = fixture
        .store
        .update(
            Some(0),
            &[
                Update::Databases(&databases, Verification::Verified),
                Update::Account(&account, Verification::Verified),
            ],
        )
        .unwrap();
    assert_eq!(saved.revision(), 1);
    assert_eq!(saved.database_keys(), databases);
    assert_eq!(saved.account_key(), Some(account.as_slice()));
    assert_eq!(saved.verified_account_key(), Some(account.as_slice()));
    let before = fs::read(fixture.store.path()).unwrap();
    assert!(fixture
        .store
        .update(
            Some(1),
            &[
                Update::Account(&[0x42; 32], Verification::Verified),
                Update::Databases(
                    &HashMap::from([("../bad.db".into(), "41".repeat(32))]),
                    Verification::Verified
                ),
            ]
        )
        .is_err());
    assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
    assert_eq!(fixture.store.load().unwrap().revision(), 1);
}

#[test]
fn unchanged_update_and_stale_revision_preserve_ciphertext() {
    let fixture = Fixture::new();
    let key = [0x51; 32];
    fixture
        .store
        .update(Some(0), &[Update::Account(&key, Verification::Verified)])
        .unwrap();
    let before = fs::read(fixture.store.path()).unwrap();
    assert_eq!(
        fixture
            .store
            .update(Some(1), &[Update::Account(&key, Verification::Verified)])
            .unwrap()
            .revision(),
        1
    );
    assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
    assert!(matches!(
        fixture.store.update(
            Some(0),
            &[Update::Account(&[0x42; 32], Verification::Verified)]
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
}

#[test]
fn wrong_account_corruption_unknown_version_and_plaintext_are_not_missing() {
    let fixture = Fixture::new();
    assert!(matches!(fixture.store.load(), Err(Error::Missing)));
    fixture
        .store
        .update(
            None,
            &[Update::Account(&[0x43; 32], Verification::Verified)],
        )
        .unwrap();
    let other = fixture.root.path().join("other-db");
    fs::create_dir(&other).unwrap();
    let foreign = Store::new(
        &other,
        &fixture.root.path().join("all_keys.json"),
        fixture.store.path(),
        vec![],
    )
    .unwrap();
    assert!(matches!(foreign.load(), Err(Error::WrongAccount)));
    let mut record = fixture.store.load().unwrap().record;
    record.version = 200;
    let plain = Zeroizing::new(serde_json::to_vec(&record).unwrap());
    let encrypted = dpapi::transform(&plain, false).unwrap();
    fs::write(fixture.store.path(), [MAGIC, encrypted.as_slice()].concat()).unwrap();
    assert!(matches!(fixture.store.load(), Err(Error::Invalid)));
    fs::write(fixture.store.path(), [MAGIC, b"corrupt"].concat()).unwrap();
    assert!(matches!(fixture.store.load(), Err(Error::Protection)));
    fs::write(
        fixture.store.path(),
        br#"{"contact/contact.db":"plaintext"}"#,
    )
    .unwrap();
    assert!(matches!(
        fixture.store.load(),
        Err(Error::LegacyMigrationRequired)
    ));
}

#[test]
fn bad_database_paths_and_lengths_cannot_replace_valid_material() {
    let fixture = Fixture::new();
    fixture
        .store
        .update(
            None,
            &[Update::Account(&[0x41; 32], Verification::Verified)],
        )
        .unwrap();
    let before = fs::read(fixture.store.path()).unwrap();
    for name in [
        "../contact.db",
        "C:/contact.db",
        "/contact.db",
        "contact//x.db",
        "a:stream.db",
        "contact/not-a-db",
        "folder./contact.db",
        "folder /contact.db",
        "CON.db",
        "LPT1/contact.db",
        "a?/contact.db",
    ] {
        let keys = HashMap::from([(name.into(), "31".repeat(32))]);
        assert!(fixture
            .store
            .update(None, &[Update::Databases(&keys, Verification::Verified)])
            .is_err());
    }
    assert!(fixture
        .store
        .update(None, &[Update::Account(&[0; 31], Verification::Verified)])
        .is_err());
    assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
}

#[test]
fn concurrent_update_lock_and_atomic_publication_failure_preserve_old_keys() {
    let fixture = Fixture::new();
    fixture
        .store
        .update(
            None,
            &[Update::Account(&[0x42; 32], Verification::Verified)],
        )
        .unwrap();
    let lock = ConfigLock::acquire(&fixture.store.path.with_extension("key-update.lock")).unwrap();
    let result = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                fixture.store.update(
                    None,
                    &[Update::Account(&[0x43; 32], Verification::Verified)],
                )
            })
            .join()
            .unwrap()
    });
    assert!(matches!(result, Err(Error::Busy)));
    drop(lock);
    let before = fs::read(fixture.store.path()).unwrap();
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(fixture.store.path())
        .unwrap();
    assert!(fixture
        .store
        .update(
            None,
            &[Update::Account(&[0x43; 32], Verification::Verified)]
        )
        .is_err());
    drop(file);
    assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
    assert_eq!(
        fixture.store.load().unwrap().account_key().unwrap(),
        &[0x42; 32]
    );
    assert!(
        !fs::read_dir(fixture.root.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp"))
    );
}

#[test]
fn absent_image_material_keeps_defaults_and_complete_image_updates_preserve_aes() {
    let fixture = Fixture::new();
    let snapshot = fixture
        .store
        .update(
            Some(0),
            &[Update::Account(&[0x42; 32], Verification::Unverified)],
        )
        .unwrap();
    assert_eq!(snapshot.image_material(), (None, 0x88));
    let snapshot = fixture
        .store
        .update(
            Some(1),
            &[Update::Image(
                b"syntheticAESkey1",
                0x89,
                Verification::Verified,
            )],
        )
        .unwrap();
    assert_eq!(snapshot.image_key(), Some((*b"syntheticAESkey1", 0x89)));
    let snapshot = fixture
        .store
        .update(
            Some(2),
            &[Update::Image(
                b"syntheticAESkey1",
                0x90,
                Verification::Verified,
            )],
        )
        .unwrap();
    assert_eq!(snapshot.image_key(), Some((*b"syntheticAESkey1", 0x90)));
    assert_eq!(snapshot.revision(), 3);
}

#[test]
fn protected_one_byte_image_records_are_invalid_without_fallback_or_file_changes() {
    for verification in [Verification::Verified, Verification::Unverified] {
        let fixture = Fixture::new();
        let snapshot = fixture
            .store
            .update(
                Some(0),
                &[Update::Image(b"syntheticAESkey1", 0xa2, verification)],
            )
            .unwrap();
        let mut record = snapshot.record;
        record.image_key.as_mut().unwrap().bytes = vec![0xa2];
        let plain = Zeroizing::new(serde_json::to_vec(&record).unwrap());
        let encrypted = dpapi::transform(&plain, false).unwrap();
        let decoded: Record =
            serde_json::from_slice(&dpapi::transform(&encrypted, true).unwrap()).unwrap();
        assert_eq!(decoded.account, fixture.store.binding);
        assert_eq!(decoded.image_key.as_ref().unwrap().bytes, [0xa2]);
        let bytes = [MAGIC, encrypted.as_slice()].concat();
        FileSnapshot::capture(fixture.store.path())
            .unwrap()
            .write_bytes(&bytes, &fixture.store.protected)
            .unwrap();
        crate::private_file::assert_private_acl(fixture.store.path());

        let old_files = [
            (
                fixture.root.path().join("all_keys.json"),
                br#"{"contact/contact.db":"synthetic"}"#.to_vec(),
            ),
            (
                fixture.root.path().join("account_key.dpapi"),
                b"synthetic old account material".to_vec(),
            ),
            (
                fixture.root.path().join("config.json"),
                br#"{"image_xor_key":162}"#.to_vec(),
            ),
        ];
        for (path, contents) in &old_files {
            fs::write(path, contents).unwrap();
        }
        assert!(matches!(fixture.store.load(), Err(Error::Invalid)));
        // Invalid is not Missing: callers must not initialize or fall back to old files.
        assert!(matches!(
            fixture.store.update(
                Some(record.revision),
                &[Update::Image(
                    b"syntheticAESkey1",
                    0xa2,
                    Verification::Verified
                )],
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(fs::read(fixture.store.path()).unwrap(), bytes);
        for (path, contents) in old_files {
            assert_eq!(fs::read(path).unwrap(), contents);
        }
    }
}
