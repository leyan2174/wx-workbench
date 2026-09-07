use super::*;

#[test]
fn verified_digest_recovers_key_without_original_audio() {
    let config = ConfigIdentity::new("local", "base-digest", "zh", "cpu").unwrap();
    let direct = CacheKey::new("u", "s", 1, b"synthetic", &config).unwrap();
    let recovered =
        CacheKey::from_audio_sha256("u", "s", 1, &digest(b"synthetic").to_uppercase(), &config)
            .unwrap();
    assert_eq!(direct.0, recovered.0);
    assert!(CacheKey::from_audio_sha256("u", "s", 1, "unknown", &config).is_err());
}

#[test]
fn two_instances_merge_and_malformed_same_key_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let a = Cache::open(&path, "a").unwrap();
    let b = Cache::open(&path, "a").unwrap();
    let k1 = key(b"a", "s", "m");
    let k2 = key(b"b", "s", "m");
    a.store_success(&k1, &record("one")).unwrap();
    b.store_success(&k2, &record("two")).unwrap();
    assert!(a.lookup(&k1).unwrap().is_some());
    assert!(a.lookup(&k2).unwrap().is_some());
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["entries"][&k1.0] = json!({"future":true});
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(a.store_success(&k1, &record("overwrite")).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
fn key(audio: &[u8], source: &str, model: &str) -> CacheKey {
    CacheKey::new(
        "synthetic-contact",
        source,
        1,
        audio,
        &ConfigIdentity::new("whisper_cpp", model, "zh", "threads=2").unwrap(),
    )
    .unwrap()
}
fn record(text: &str) -> CachedTranscription {
    CachedTranscription {
        text: text.into(),
        language: "zh".into(),
        create_time: Some(123),
    }
}

#[test]
fn persists_empty_success_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "account-a").unwrap();
    let key = key(b"audio", "s0", "model");
    assert!(cache.lookup(&key).unwrap().is_none());
    assert!(!path.exists());
    assert_eq!(
        cache.store_success(&key, &record("")).unwrap(),
        StoreStatus::Stored
    );
    assert!(
        Cache::open(&path, "account-a")
            .unwrap()
            .lookup(&key)
            .unwrap()
            == Some(record(""))
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn isolates_accounts_audio_source_and_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "account-a").unwrap();
    cache
        .store_success(&key(b"audio", "s0", "model"), &record("ok"))
        .unwrap();
    assert!(Cache::open(&path, "account-b").is_err());
    for k in [
        key(b"changed", "s0", "model"),
        key(b"audio", "s1", "model"),
        key(b"audio", "s0", "new-model"),
    ] {
        assert!(cache.lookup(&k).unwrap().is_none());
    }
    let config = ConfigIdentity::new("openai", "model", "en", "endpoint-version").unwrap();
    assert!(cache
        .lookup(&CacheKey::new("synthetic-contact", "s0", 1, b"audio", &config).unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn protects_existing_records_and_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "a").unwrap();
    let k = key(b"audio", "s", "m");
    cache.store_success(&k, &record("keep")).unwrap();
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["extension"] = json!({"keep":42});
    value["entries"][&k.0]["custom"] = json!([1, 2]);
    value["entries"]["opaque"] = json!(null);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        cache.store_success(&k, &record("replace")).unwrap(),
        StoreStatus::AlreadyPresent
    );
    cache
        .store_success(&key(b"new", "s", "m"), &record("new"))
        .unwrap();
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["extension"], value["extension"]);
    assert_eq!(after["entries"][&k.0], value["entries"][&k.0]);
    assert!(after["entries"].get("opaque").unwrap().is_null());
}

#[test]
fn legacy_and_corrupt_files_are_not_claimed_or_destroyed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    for data in [
        "{broken",
        "[]",
        r#"{"[\"u\", 1]":{"text":"old","model_size":"base"}}"#,
    ] {
        fs::write(&path, data).unwrap();
        assert!(Cache::open(&path, "a").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), data);
    }
}

#[test]
fn stale_publication_and_cooperative_lock_preserve_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "a").unwrap();
    let k = key(b"a", "s", "m");
    cache.store_success(&k, &record("ok")).unwrap();
    let (before, value) = cache.load().unwrap();
    fs::write(&path, "external").unwrap();
    assert!(publish(&path, before, &value, || Ok(())).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    let _lock = Lock::acquire(&path).unwrap();
    assert!(cache.store_success(&k, &record("no")).is_err());
}

#[test]
fn denied_commit_preserves_new_and_existing_cache_and_cleans_staging() {
    for existing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let cache = Cache::open(&path, "a").unwrap();
        if existing {
            cache
                .store_success(&key(b"old", "s", "m"), &record("keep"))
                .unwrap();
        }
        let before = fs::read(&path).ok();
        let mut called = false;
        let result = cache.store_success_checked(&key(b"new", "s", "m"), &record("reject"), || {
            called = true;
            // 钩子确实在暂存文件写完后、目标发布前执行。
            assert!(fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wx-cache-")));
            anyhow::bail!("synthetic commit denial")
        });
        assert!(result.is_err());
        assert!(called);
        assert_eq!(fs::read(&path).ok(), before);
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            usize::from(existing)
        );
    }
}
