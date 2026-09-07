use super::cache::*;
use serde_json::{json, Value};
use std::{fs, path::Path};

fn key(audio: &[u8]) -> CacheKey {
    CacheKey::new(
        "synthetic-user",
        "message/media_0.db",
        1,
        audio,
        &ConfigIdentity::new("local", "synthetic-model-sha", "zh", "synthetic-options").unwrap(),
    )
    .unwrap()
}
fn record() -> CachedTranscription {
    CachedTranscription {
        text: "synthetic transcript".into(),
        language: "zh".into(),
        create_time: Some(100),
    }
}
fn contents(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn identity_plaintext_is_not_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "synthetic-account-identity").unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    cache
        .store_success(&key(b"synthetic-raw-audio"), &record())
        .unwrap();
    // 后端失败及错误凭据隔离由真实 cached 路径测试，不制造缓存专用失败 API。
    let text = fs::read_to_string(&path).unwrap();
    for secret in [
        "synthetic-account-identity",
        "synthetic-raw-audio",
        "synthetic-user",
        "synthetic-model-sha",
        "synthetic-options",
    ] {
        assert!(!text.contains(secret), "泄露 {secret}");
    }
}

#[test]
fn malformed_or_wrong_account_replacement_after_open_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "a").unwrap();
    for replacement in [
        b"{broken".to_vec(),
        serde_json::to_vec(
            &json!({"_wx_asr_cache":{"version":1,"account_sha256":"another-account"},"entries":{}}),
        )
        .unwrap(),
        b"[]".to_vec(),
    ] {
        fs::write(&path, &replacement).unwrap();
        assert!(cache.lookup(&key(b"audio")).is_err());
        assert!(cache.store_success(&key(b"audio"), &record()).is_err());
        assert_eq!(fs::read(&path).unwrap(), replacement);
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "锁和临时文件不得残留"
        );
    }
}

#[test]
fn envelope_versions_and_entries_are_validated_without_claiming_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    Cache::open(&path, "a")
        .unwrap()
        .store_success(&key(b"a"), &record())
        .unwrap();
    let base = contents(&path);
    for field in ["version", "account_sha256", "entries"] {
        let mut value = base.clone();
        if field == "entries" {
            value[field] = json!([]);
        } else {
            value["_wx_asr_cache"]
                .as_object_mut()
                .unwrap()
                .remove(field);
        }
        let before = serde_json::to_vec(&value).unwrap();
        fs::write(&path, &before).unwrap();
        assert!(Cache::open(&path, "a").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn unknown_large_numbers_and_nested_fields_survive_new_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "a").unwrap();
    cache.store_success(&key(b"a"), &record()).unwrap();
    let mut before = contents(&path);
    let unknown:Value=serde_json::from_str(r#"{"huge":184467440737095516160000000001,"precise":0.12345678901234567890123456789,"nested":[null,{"future":true}]}"#).unwrap();
    before["future"] = unknown.clone();
    before["_wx_asr_cache"]["future"] = unknown.clone();
    let first = before["entries"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    before["entries"][&first]["future"] = unknown;
    fs::write(&path, serde_json::to_vec(&before).unwrap()).unwrap();
    cache.store_success(&key(b"b"), &record()).unwrap();
    let after = contents(&path);
    assert_eq!(after["future"], before["future"]);
    assert_eq!(after["_wx_asr_cache"], before["_wx_asr_cache"]);
    assert_eq!(after["entries"][&first], before["entries"][&first]);
}

#[test]
fn oversize_file_is_rejected_by_read_limit_before_json_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let cache = Cache::open(&path, "a").unwrap();
    // 小型稀疏/零填充合成文件；JSON 本身无效，必须优先得到读取限额错误。
    let size = 64 * 1024 * 1024 + 4096;
    fs::File::create(&path).unwrap().set_len(size).unwrap();
    let error = cache.lookup(&key(b"a")).err().unwrap().to_string();
    assert!(error.contains("cache exceeds 64 MiB"), "{error}");
    assert!(cache.store_success(&key(b"a"), &record()).is_err());
    assert_eq!(fs::metadata(&path).unwrap().len(), size);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn hardlink_publication_replaces_cache_name_not_alias_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let alias = dir.path().join("alias.json");
    let cache = Cache::open(&path, "a").unwrap();
    cache.store_success(&key(b"a"), &record()).unwrap();
    fs::hard_link(&path, &alias).unwrap();
    let before = fs::read(&alias).unwrap();
    assert!(same_file::is_same_file(&path, &alias).unwrap());
    cache.store_success(&key(b"b"), &record()).unwrap();
    assert_eq!(fs::read(&alias).unwrap(), before);
    assert!(!same_file::is_same_file(&path, &alias).unwrap());
    assert!(cache.lookup(&key(b"b")).unwrap().is_some());
    assert!(Cache::open(&alias, "a")
        .unwrap()
        .lookup(&key(b"b"))
        .unwrap()
        .is_none());
}

#[test]
fn stale_cooperative_lock_is_preserved_and_never_stolen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Cache.JSON");
    let cache = Cache::open(&path, "a").unwrap();
    let lock = dir.path().join(".cache.json.asr-cache.lock");
    fs::write(&lock, b"synthetic stale lock").unwrap();
    assert!(cache.store_success(&key(b"a"), &record()).is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"synthetic stale lock");
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
#[ignore = "当前宿主创建文件符号链接返回 Windows 1314；需显式权限环境重跑"]
fn final_symlink_is_rejected_without_touching_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.json");
    let path = dir.path().join("cache.json");
    Cache::open(&target, "a")
        .unwrap()
        .store_success(&key(b"a"), &record())
        .unwrap();
    let before = fs::read(&target).unwrap();
    std::os::windows::fs::symlink_file(&target, &path)
        .expect("需要 Windows 开发者模式或创建符号链接权限；不能静默跳过此攻击用例");
    assert!(Cache::open(&path, "a").is_err());
    assert_eq!(fs::read(&target).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn final_directory_junction_is_rejected_without_touching_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let path = dir.path().join("cache.json");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"untouched").unwrap();
    let mut command = std::process::Command::new("cmd.exe");
    command
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&path)
        .arg(&target);
    println!("COMMAND: {command:?}");
    let output = command.output().unwrap();
    println!(
        "STDOUT: {}\nSTDERR: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    let rejected = Cache::open(&path, "a").is_err();
    // RemoveDirectory 仅移除 junction 本身，不递归触碰指向的目录。
    fs::remove_dir(&path).unwrap();
    assert!(rejected);
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched");
}
