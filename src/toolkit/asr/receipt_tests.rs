use super::super::{cache::StoreStatus, local::LocalConfig, openai::OpenAiConfig};
use super::*;
use std::{fs, time::Duration};

const SILK: &[u8] = include_bytes!("../../../tests/fixtures/audio/silence.silk");

fn evidence() -> VoiceEvidence {
    VoiceEvidence {
        username: "wxid_peer".into(),
        message_source: "message/message_0.db".into(),
        message_table: format!("Msg_{:x}", md5::compute("wxid_peer")),
        message_local_id: 7,
        server_id: 100,
        create_time: 123,
        media_source: "message/media_0.db".into(),
        media_rowid: 1,
        media_chat_name_id: 9,
        media_local_id: 700,
    }
}

fn request<'a>(path: &'a Path, evidence: &'a VoiceEvidence) -> CachedRequest<'a> {
    CachedRequest {
        cache_path: path,
        account: "runtime-account-a",
        username: &evidence.username,
        source: &evidence.message_source,
        local_id: evidence.message_local_id,
        create_time: evidence.create_time,
        silk: SILK,
    }
}

fn local(dir: &Path) -> Backend {
    let program = dir.join("not-an-executable.bin");
    let model = dir.join("model.bin");
    fs::write(&program, b"synthetic non-executable identity").unwrap();
    fs::write(&model, b"synthetic model identity").unwrap();
    Backend::Local(LocalConfig::new(program, model))
}

fn record() -> CachedTranscription {
    CachedTranscription {
        text: String::new(),
        language: "zh".into(),
        create_time: Some(123),
    }
}

fn store(path: &Path, backend: &Backend, evidence: &VoiceEvidence) -> ReceiptState {
    let req = request(path, evidence);
    let proof = Proof::new(&req, evidence).unwrap();
    let (config, _files, _prepared) = cached::identity(backend, req.create_time).unwrap();
    let mut result = record();
    result.create_time = Some(evidence.create_time);
    Cache::open(path, req.account)
        .unwrap()
        .store_success_with_receipt_checked(
            &proof.key(&config).unwrap(),
            &result,
            &proof,
            &config,
            || Ok(()),
        )
        .unwrap()
        .1
}

fn find(path: &Path, backend: &Backend) -> LookupOutcome {
    lookup_success(path, "runtime-account-a", "wxid_peer", 700, backend)
}

#[test]
fn real_identity_and_cache_io_survive_deleted_source_without_running_backend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let source = dir.path().join("source.silk");
    fs::write(&source, SILK).unwrap();
    assert_eq!(store(&path, &backend, &evidence()), ReceiptState::Stored);
    fs::remove_file(&source).unwrap();
    let before = fs::read(&path).unwrap();
    let LookupOutcome::Hit(hit) = find(&path, &backend) else {
        panic!("expected cache-only hit")
    };
    assert_eq!(hit.transcription.text, "");
    assert_eq!(hit.transcription.language, "zh");
    assert_eq!(hit.transcription.backend, "whisper_cpp");
    assert_eq!(hit.create_time, 123);
    assert_eq!(hit.cache_state, CacheState::Hit);
    assert_eq!(before, fs::read(&path).unwrap());
    assert!(!source.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    let data: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(data["_wx_asr_cache"]["version"], 1);
    assert_eq!(data[FIELD]["version"], 1);
    assert_eq!(data["entries"].as_object().unwrap().len(), 1);
}

#[test]
fn exact_account_username_media_and_existing_keys_are_not_weakened() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    store(&path, &backend, &e);
    for (user, id) in [
        ("peer", 700),
        ("WXID_PEER", 700),
        ("wxid_peer", 7),
        ("other", 700),
    ] {
        assert!(matches!(
            lookup_success(&path, "runtime-account-a", user, id, &backend),
            LookupOutcome::Miss
        ));
    }
    assert!(matches!(
        lookup_success(&path, "runtime-account-b", "wxid_peer", 700, &backend),
        LookupOutcome::Unavailable
    ));
    for id in [0, -1, i64::MIN] {
        assert!(matches!(
            lookup_success(&path, "runtime-account-a", "wxid_peer", id, &backend),
            LookupOutcome::Unavailable
        ));
    }
    let (config, _files, _prepared) = cached::identity(&backend, 123).unwrap();
    let key = CacheKey::new(&e.username, &e.message_source, 7, SILK, &config).unwrap();
    assert!(
        Cache::open(&path, "runtime-account-a")
            .unwrap()
            .lookup(&key)
            .unwrap()
            == Some(record())
    );
}

#[test]
fn current_local_config_changes_miss_but_timeout_changes_hit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    store(&path, &backend, &evidence());
    let Backend::Local(original) = backend else {
        panic!()
    };
    for field in 0..4 {
        let mut changed = original.clone();
        match field {
            0 => changed.threads += 1,
            1 => changed.language = "en".into(),
            2 => changed.output_format = super::super::local::OutputFormat::Json,
            _ => {
                let copy = dir.path().join("same-content-different-path.bin");
                fs::copy(&changed.executable, &copy).unwrap();
                changed.executable = copy;
            }
        }
        assert!(matches!(
            find(&path, &Backend::Local(changed)),
            LookupOutcome::Miss
        ));
    }
    let mut changed = original.clone();
    changed.timeout = Duration::from_millis(1);
    assert!(matches!(
        find(&path, &Backend::Local(changed)),
        LookupOutcome::Hit(_)
    ));
    for input in [&original.executable, &original.model] {
        let before = fs::read(input).unwrap();
        fs::write(input, b"changed bytes at same path").unwrap();
        assert!(matches!(
            find(&path, &Backend::Local(original.clone())),
            LookupOutcome::Miss
        ));
        fs::write(input, before).unwrap();
    }
    fs::remove_file(&original.model).unwrap();
    assert!(matches!(
        find(&path, &Backend::Local(original)),
        LookupOutcome::Unavailable
    ));
}

#[test]
fn multiple_configs_share_proof_but_every_strong_evidence_change_marks_conflict() {
    for field in 0..10 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let backend = local(dir.path());
        let original = evidence();
        store(&path, &backend, &original);
        let Backend::Local(config) = &backend else {
            panic!()
        };
        let mut second = config.clone();
        second.language = "en".into();
        let second = Backend::Local(second);
        assert_eq!(store(&path, &second, &original), ReceiptState::Stored);
        assert!(matches!(find(&path, &backend), LookupOutcome::Hit(_)));
        assert!(matches!(find(&path, &second), LookupOutcome::Hit(_)));
        let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut changed = original.clone();
        match field {
            0 => changed.message_source = "message/message_1.db".into(),
            1 => changed.message_local_id += 1,
            2 => changed.server_id += 1,
            3 => changed.create_time += 1,
            4 => changed.media_source = "message/media_1.db".into(),
            5 => changed.media_rowid += 1,
            6 => changed.media_chat_name_id += 1,
            _ => {}
        }
        let mut req = request(&path, &changed);
        if field == 7 {
            req.silk = include_bytes!("../../../tests/fixtures/audio/tone.silk");
        }
        let mut proof = Proof::new(&req, &changed).unwrap();
        if field == 8 {
            proof.audio_sha256 = "00".repeat(32);
        }
        if field == 9 {
            proof.audio_bytes += 1;
        }
        let (identity, _files, _prepared) =
            cached::identity(&backend, changed.create_time).unwrap();
        let mut result = record();
        result.create_time = Some(changed.create_time);
        let (_, state) = Cache::open(&path, req.account)
            .unwrap()
            .store_success_with_receipt_checked(
                &proof.key(&identity).unwrap(),
                &result,
                &proof,
                &identity,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(state, ReceiptState::Conflict, "field {field}");
        assert!(matches!(find(&path, &backend), LookupOutcome::Conflict));
        assert!(matches!(find(&path, &second), LookupOutcome::Conflict));
        assert_eq!(store(&path, &backend, &original), ReceiptState::Conflict);
        let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let key = index_key("wxid_peer", 700).unwrap();
        assert_eq!(
            before[FIELD]["entries"][&key]["proof"],
            after[FIELD]["entries"][&key]["proof"]
        );
        for (key, entry) in before["entries"].as_object().unwrap() {
            assert_eq!(*entry, after["entries"][key]);
        }
    }
}

#[test]
fn atomic_denial_preserves_entries_and_index_and_cleans_staging() {
    for exists in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let backend = local(dir.path());
        let e = evidence();
        let req = request(&path, &e);
        let proof = Proof::new(&req, &e).unwrap();
        let (config, _files, _prepared) = cached::identity(&backend, e.create_time).unwrap();
        let cache = Cache::open(&path, req.account).unwrap();
        let key = proof.key(&config).unwrap();
        if exists {
            cache.store_success(&key, &record()).unwrap();
        }
        let before = fs::read(&path).ok();
        let mut calls = 0;
        assert!(cache
            .store_success_with_receipt_checked(&key, &record(), &proof, &config, || {
                calls += 1;
                assert_eq!(before, fs::read(&path).ok());
                anyhow::bail!("synthetic denial")
            })
            .is_err());
        assert_eq!(calls, 1);
        assert_eq!(before, fs::read(&path).ok());
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            2 + usize::from(exists)
        );
        assert_eq!(store(&path, &backend, &e), ReceiptState::Stored);
        let complete = fs::read(&path).unwrap();
        assert_eq!(
            cache
                .store_success_with_receipt_checked(&key, &record(), &proof, &config, || {
                    panic!("idempotent receipt must not publish")
                })
                .unwrap(),
            (StoreStatus::AlreadyPresent, ReceiptState::AlreadyPresent)
        );
        assert_eq!(complete, fs::read(path).unwrap());
    }
}

#[test]
fn malformed_index_and_record_digest_fail_closed_without_rewriting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    store(&path, &backend, &e);
    let base: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for field in 0..9 {
        let mut data = base.clone();
        let key = index_key("wxid_peer", 700).unwrap();
        match field {
            0 => data[FIELD]["version"] = serde_json::json!(2),
            1 => data[FIELD]["future"] = serde_json::json!(true),
            2 => {
                data[FIELD]["entries"][&key]["proof"]["audio_bytes"] =
                    serde_json::json!(MAX_VOICE_BYTES + 1)
            }
            3 => data[FIELD]["entries"][&key]["proof"]["audio_sha256"] = serde_json::json!("bad"),
            4 => {
                data[FIELD]["entries"][&key]["proof"]["evidence"]["username"] =
                    serde_json::json!("other")
            }
            5 => {
                data[FIELD]["entries"][&key]["proof"]["evidence"]["message_source"] =
                    serde_json::json!("../message_0.db")
            }
            _ => {
                let entry = data["entries"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap();
                match field {
                    6 => entry["text"] = serde_json::json!("tampered"),
                    7 => entry["language"] = serde_json::json!("en"),
                    _ => entry["create_time"] = Value::Null,
                }
            }
        }
        let bytes = serde_json::to_vec(&data).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(find(&path, &backend), LookupOutcome::Unavailable),
            "field {field}"
        );
        assert_eq!(bytes, fs::read(&path).unwrap());
    }
    let mut data = base;
    data["entries"] = serde_json::json!({});
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(matches!(find(&path, &backend), LookupOutcome::Miss));
}

#[test]
fn old_strong_cache_can_be_enriched_but_weak_cache_is_never_imported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    let req = request(&path, &e);
    let proof = Proof::new(&req, &e).unwrap();
    let (config, _files, _prepared) = cached::identity(&backend, 123).unwrap();
    let cache = Cache::open(&path, req.account).unwrap();
    cache
        .store_success(&proof.key(&config).unwrap(), &record())
        .unwrap();
    assert!(matches!(find(&path, &backend), LookupOutcome::Miss));
    let mut data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    data["unrelated_extension"] = serde_json::json!({"keep": [1,2,3]});
    data["entries"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["future"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(store(&path, &backend, &e), ReceiptState::Stored);
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["entries"], data["entries"]);
    assert_eq!(after["unrelated_extension"], data["unrelated_extension"]);
    for old in [b"{broken".as_slice(), b"{\"weak-id\":{\"text\":\"old\"}}"] {
        fs::write(&path, old).unwrap();
        assert!(matches!(find(&path, &backend), LookupOutcome::Unavailable));
        assert_eq!(fs::read(&path).unwrap(), old);
    }
}

#[test]
fn existing_different_record_never_gets_certified_by_new_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    let req = request(&path, &e);
    let proof = Proof::new(&req, &e).unwrap();
    let (config, _files, _prepared) = cached::identity(&backend, 123).unwrap();
    let cache = Cache::open(&path, req.account).unwrap();
    let key = proof.key(&config).unwrap();
    let mut old = record();
    old.text = "already stored".into();
    cache.store_success(&key, &old).unwrap();
    let (_, state) = cache
        .store_success_with_receipt_checked(&key, &record(), &proof, &config, || Ok(()))
        .unwrap();
    assert_eq!(state, ReceiptState::Conflict);
    assert!(cache.lookup(&key).unwrap() == Some(old));
    assert!(matches!(find(&path, &backend), LookupOutcome::Conflict));
}

#[test]
fn ordinary_cache_writers_preserve_receipts_and_shared_lock_prevents_split_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    let req = request(&path, &e);
    let a = Cache::open(&path, req.account).unwrap();
    let b = Cache::open(&path, req.account).unwrap();
    store(&path, &backend, &e);
    let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let (config, _files, _prepared) = cached::identity(&backend, 123).unwrap();
    let other = CacheKey::new("other", "message/message_0.db", 7, SILK, &config).unwrap();
    b.store_success(&other, &record()).unwrap();
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(before[FIELD], after[FIELD]);
    assert!(matches!(find(&path, &backend), LookupOutcome::Hit(_)));
    let mut changed = e.clone();
    changed.media_local_id = 701;
    let proof = Proof::new(&request(&path, &changed), &changed).unwrap();
    let lock = dir.path().join(".cache.json.asr-cache.lock");
    fs::write(&lock, b"other writer").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(a
        .store_success_with_receipt_checked(
            &proof.key(&config).unwrap(),
            &record(),
            &proof,
            &config,
            || { panic!("locked publication cannot call commit hook") }
        )
        .is_err());
    assert_eq!(before, fs::read(&path).unwrap());
    assert_eq!(fs::read(&lock).unwrap(), b"other writer");
    fs::remove_file(lock).unwrap();
    assert_eq!(store(&path, &backend, &changed), ReceiptState::Stored);
    assert!(matches!(
        lookup_success(&path, req.account, "wxid_peer", 701, &backend),
        LookupOutcome::Hit(_)
    ));
}

#[test]
fn real_cloud_authorization_precedes_io_and_current_identity_does_not_upload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let make = |version: &str, model: &str, language: Option<&str>, limit: usize, key: &str| {
        Backend::explicit_openai(
            OpenAiConfig {
                base_url: format!("{base}/{version}"),
                model: model.into(),
                language: language.map(str::to_owned),
                api_key: key.into(),
                timeout: Duration::from_millis(100),
                max_audio_bytes: limit,
            },
            true,
        )
        .unwrap()
    };
    let mut backend = make("v1", "model-a", Some("zh"), 1024, "SYNTHETIC_SECRET");
    store(&path, &backend, &evidence());
    assert!(matches!(find(&path, &backend), LookupOutcome::Hit(_)));
    assert!(matches!(
        find(
            &path,
            &make("v1", "model-a", Some("zh"), 1024, "ROTATED_SECRET")
        ),
        LookupOutcome::Hit(_)
    ));
    for other in [
        make("v2", "model-a", Some("zh"), 1024, "key"),
        make("v1", "model-b", Some("zh"), 1024, "key"),
        make("v1", "model-a", None, 1024, "key"),
        make("v1", "model-a", Some("en"), 1024, "key"),
        make("v1", "model-a", Some("zh"), 512, "key"),
    ] {
        assert!(matches!(find(&path, &other), LookupOutcome::Miss));
    }
    let Backend::ExplicitOpenAi { allow_upload, .. } = &mut backend else {
        panic!()
    };
    *allow_upload = false;
    assert!(matches!(find(&path, &backend), LookupOutcome::Unavailable));
    let error = lookup(&dir.path().join("absent/cache.json"), "", "", -1, &backend)
        .err()
        .unwrap();
    assert!(error.to_string().contains("authorization"));
    assert_eq!(
        listener.accept().err().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let text = fs::read_to_string(path).unwrap();
    for secret in [
        "SYNTHETIC_SECRET",
        "ROTATED_SECRET",
        base.as_str(),
        "model-a",
    ] {
        assert!(!text.contains(secret));
    }
}

#[cfg(windows)]
#[test]
fn real_identity_holds_model_and_program_handles_through_atomic_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let backend = local(dir.path());
    let e = evidence();
    let req = request(&path, &e);
    let proof = Proof::new(&req, &e).unwrap();
    let (config, files, _prepared) = cached::identity(&backend, 123).unwrap();
    let Backend::Local(local) = &backend else {
        panic!()
    };
    Cache::open(&path, req.account)
        .unwrap()
        .store_success_with_receipt_checked(
            &proof.key(&config).unwrap(),
            &record(),
            &proof,
            &config,
            || {
                for input in [&local.model, &local.executable] {
                    assert!(fs::OpenOptions::new().write(true).open(input).is_err());
                    assert!(fs::remove_file(input).is_err());
                }
                Ok(())
            },
        )
        .unwrap();
    drop(files);
    fs::OpenOptions::new()
        .write(true)
        .open(&local.model)
        .unwrap();
}
