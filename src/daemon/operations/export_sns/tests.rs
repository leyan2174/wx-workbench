use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn case(name: &str) -> (Vec<u8>, Vec<u8>, CacheKeys) {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/sns/cache_golden.json"
    ))
    .unwrap();
    let case = golden["decrypt_cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap();
    (
        bytes(case["input_hex"].as_str().unwrap()),
        bytes(case["expected_hex"].as_str().unwrap()),
        CacheKeys {
            image_aes_key: Some(bytes(case["key_hex"].as_str().unwrap()).try_into().unwrap()),
            image_xor_key: case["xor_key"].as_u64().unwrap() as u8,
        },
    )
}

fn candidate(root: &Path, xwechat: bool, input: &[u8]) -> PathBuf {
    let path = root.join(if xwechat {
        "2026-09/Sns/Img/00/fixture"
    } else {
        "2026-09/fixture"
    });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, input).unwrap();
    path
}

fn assert_decoded(path: &Path, keys: &CacheKeys, expected: &[u8]) {
    assert_eq!(
        crate::adapters::wechat::moments::cache::decrypt_dat(
            &fs::read(path).unwrap(),
            keys,
            1024 * 1024
        )
        .unwrap(),
        expected
    );
}

#[test]
fn stored_aes_with_plain_cache_needs_no_v2_sample() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("plain");
    let (input, expected, keys) = case("plain_png");
    let path = candidate(&root, true, &input);
    let mut sources = source::Sources::default();
    let roots = CacheRoots {
        xwechat: Some(sources.directory(&root).unwrap()),
        file_storage_sns: None,
    };
    let (index, samples) = guarded_cache(&roots, &keys, &mut sources).unwrap();
    assert_eq!(index.images().len(), 1);
    assert!(samples.is_empty());
    assert_decoded(&path, &keys, &expected);
    assert!(fs::write(&path, b"replacement").is_err());
    sources.verify().unwrap();
    drop(sources);
    fs::write(path, b"released").unwrap();
}

#[test]
fn xor_only_cache_remains_usable_without_aes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("xor");
    let (input, expected, mut keys) = case("xor_png");
    keys.image_aes_key = None;
    let path = candidate(&root, false, &input);
    let mut sources = source::Sources::default();
    let roots = CacheRoots {
        xwechat: None,
        file_storage_sns: Some(sources.directory(&root).unwrap()),
    };
    let (index, samples) = guarded_cache(&roots, &keys, &mut sources).unwrap();
    assert_eq!(index.images().len(), 1);
    assert!(samples.is_empty());
    assert_decoded(&path, &keys, &expected);
    sources.verify().unwrap();
}

#[test]
fn mixed_roots_validate_v2_but_do_not_require_aes_evidence_from_plain_root() {
    let temp = tempfile::tempdir().unwrap();
    let encrypted = temp.path().join("encrypted");
    let plain = temp.path().join("plain");
    let (input, expected, keys) = case("v2_aes_96");
    let v2 = candidate(&encrypted, true, &input);
    let (input, plain_expected, _) = case("plain_png");
    let plain_path = candidate(&plain, false, &input);
    let mut sources = source::Sources::default();
    let roots = CacheRoots {
        xwechat: Some(sources.directory(&encrypted).unwrap()),
        file_storage_sns: Some(sources.directory(&plain).unwrap()),
    };
    let (index, samples) = guarded_cache(&roots, &keys, &mut sources).unwrap();
    assert_eq!(index.images().len(), 2);
    assert_eq!(samples.len(), 1);
    samples[0].verify().unwrap();
    assert_decoded(&v2, &keys, &expected);
    assert_decoded(&plain_path, &keys, &plain_expected);
    for path in [&v2, &plain_path] {
        assert!(fs::remove_file(path).is_err());
    }
    sources.verify().unwrap();
}

#[test]
fn later_invalid_v2_is_fatal_after_a_valid_candidate_not_a_skipped_warning() {
    for truncated in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let later = temp.path().join("later");
        let (input, _, keys) = case("v2_aes_96");
        let good = candidate(&first, true, &input);
        let mut invalid = input;
        if truncated {
            invalid.truncate(20);
        } else {
            invalid[15..31].fill(0);
        }
        let bad = candidate(&later, false, &invalid);
        let mut sources = source::Sources::default();
        let roots = CacheRoots {
            xwechat: Some(sources.directory(&first).unwrap()),
            file_storage_sns: Some(sources.directory(&later).unwrap()),
        };
        // Xwechat is enumerated before the legacy root: a valid sample must not
        // authorize a different candidate whose ciphertext cannot be validated.
        let error = guarded_cache(&roots, &keys, &mut sources).err().unwrap();
        assert!(error.to_string().contains("材料未通过验证"));
        for path in [&good, &bad] {
            assert!(fs::remove_file(path).is_err());
        }
        sources.verify().unwrap();
        drop(sources);
        fs::remove_file(good).unwrap();
        fs::remove_file(bad).unwrap();
    }
}

#[test]
fn every_v2_in_one_root_requires_its_own_valid_sample() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("encrypted");
    let (input, _, keys) = case("v2_aes_96");
    let first = candidate(&root, true, &input);
    let second = first.with_file_name("second");
    fs::write(&second, &input).unwrap();
    let mut sources = source::Sources::default();
    let roots = CacheRoots {
        xwechat: Some(sources.directory(&root).unwrap()),
        file_storage_sns: None,
    };
    let (index, samples) = guarded_cache(&roots, &keys, &mut sources).unwrap();
    assert_eq!(index.images().len(), 2);
    assert_eq!(samples.len(), 2);
    for sample in &samples {
        sample.verify().unwrap();
    }
    drop(samples);
    drop(sources);
    let mut invalid = input;
    invalid.truncate(20);
    fs::write(second, invalid).unwrap();
    let mut sources = source::Sources::default();
    assert!(guarded_cache(&roots, &keys, &mut sources).is_err());
}

#[test]
fn actual_v2_without_protected_aes_is_rejected_without_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("encrypted");
    let (input, _, mut keys) = case("v2_aes_96");
    keys.image_aes_key = None;
    candidate(&root, true, &input);
    let mut sources = source::Sources::default();
    let roots = CacheRoots {
        xwechat: Some(sources.directory(&root).unwrap()),
        file_storage_sns: None,
    };
    let error = guarded_cache(&roots, &keys, &mut sources).err().unwrap();
    assert!(error.to_string().contains("缺少受保护材料"));
}

fn offline_database(root: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let path = root.join("sns.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);")
        .unwrap();
    db.execute("INSERT INTO SnsTimeLine VALUES (1, 'test-user', ?1)", [
        "<root><LocalExtraInfo><nickname>Tester</nickname></LocalExtraInfo><TimelineObject><id>1</id><username>test-user</username><createTime>1700000000</createTime><ContentObject><type>1</type><mediaList><media><type>2</type><url>https://synthetic.invalid/image</url></media></mediaList></ContentObject></TimelineObject></root>"
    ]).unwrap();
    path
}

#[test]
fn no_cache_fresh_verification_succeeds_without_material_or_recovery_fields() {
    use crate::application::moments::{
        export_database_verified, ExportOptions, VerifiedPublication,
    };
    let temp = tempfile::tempdir().unwrap();
    let db = offline_database(&temp.path().join("source"));
    let mut sources = source::Sources::default();
    let db = sources.database(&db).unwrap();
    let output = temp.path().join("out");
    let calls = std::cell::Cell::new(0);
    let verify = || {
        calls.set(calls.get() + 1);
        sources.verify()
    };
    let report = export_database_verified(
        &db,
        None,
        &output,
        &ExportOptions::default(),
        None,
        None,
        VerifiedPublication {
            policy: None,
            verify: &verify,
        },
    )
    .unwrap();
    assert_eq!(report.posts, 1);
    assert_eq!(report.media_recovered, 0);
    assert_eq!(calls.get(), 2); // Timeline entry and immediately before directory rename.
    let root = output.join("Tester/SNS");
    assert!(!root.join("_media_recovery.json").exists());
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("timeline.json")).unwrap()).unwrap();
    assert!(value["posts"][0]["media"][0].get("local_file").is_none());
}

#[test]
fn absent_sidecar_appearing_at_actual_update_commit_keeps_old_timeline() {
    use crate::application::moments::{
        export_database_verified, ExportOptions, TimelinePublication, VerifiedPublication,
    };
    let temp = tempfile::tempdir().unwrap();
    let db = offline_database(&temp.path().join("source"));
    let mut sources = source::Sources::default();
    let db = sources.database(&db).unwrap();
    let output = temp.path().join("out");
    let publication = TimelinePublication {
        flat_cache: false,
        source_kind: "snapshot".into(),
        source_id: "synthetic-no-cache".into(),
        policy: crate::infrastructure::output_tree::ExistingPolicy::Update,
        inputs: vec![db.clone()],
    };
    let verify = || sources.verify();
    let options = ExportOptions {
        export_time: Some(1700000000),
        ..ExportOptions::default()
    };
    let report = export_database_verified(
        &db,
        None,
        &output,
        &options,
        None,
        None,
        VerifiedPublication {
            policy: Some(&publication),
            verify: &verify,
        },
    )
    .unwrap();
    let before: Vec<_> = report
        .files
        .iter()
        .map(|path| (path.clone(), fs::read(path).unwrap()))
        .collect();
    let mut sidecar = db.as_os_str().to_os_string();
    sidecar.push("-wal");
    let sidecar = PathBuf::from(sidecar);
    assert!(!sidecar.exists());
    let calls = std::cell::Cell::new(0);
    let mutate_before_commit = || -> anyhow::Result<()> {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            fs::write(&sidecar, b"synthetic new sidecar")?;
        }
        sources.verify()
    };
    let changed = ExportOptions {
        export_time: Some(1700000060),
        ..ExportOptions::default()
    };
    let error = export_database_verified(
        &db,
        None,
        &output,
        &changed,
        None,
        None,
        VerifiedPublication {
            policy: Some(&publication),
            verify: &mutate_before_commit,
        },
    )
    .unwrap_err();
    assert_eq!(calls.get(), 2);
    let message = format!("{error:#}");
    assert!(message.contains("Offline SQLite source changed"));
    assert!(message.contains("0 committed file(s)"));
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(!output.join("Tester/SNS/_media_recovery.json").exists());
}
