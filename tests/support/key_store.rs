//! Direct current-format DPAPI fixtures. Never launch acquisition or read legacy files.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[path = "../../src/key_store/dpapi.rs"]
pub(crate) mod dpapi;

#[allow(dead_code)]
pub fn seed(config: &Path, keys: &Value) {
    write(config, keys, None, "verified");
}

#[allow(dead_code)]
pub fn seed_unverified(config: &Path, keys: &Value) {
    write(config, keys, None, "unverified");
}

#[allow(dead_code)]
pub fn seed_image(config: &Path, aes: &[u8; 16], xor: u8) {
    let mut bytes = aes.to_vec();
    bytes.push(xor);
    write(config, &json!({}), Some(bytes), "verified");
}

fn normalized(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}

fn write(config: &Path, keys: &Value, image: Option<Vec<u8>>, verification: &str) {
    let temp = std::env::temp_dir().canonicalize().unwrap();
    let config = config.canonicalize().unwrap();
    assert!(
        config.starts_with(&temp),
        "fixture must be a temporary configuration"
    );
    let base = config.parent().unwrap();
    let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert!(value.get("image_aes_key").is_none() && value.get("image_xor_key").is_none());
    let resolve = |field: &str, fallback: &str| -> PathBuf {
        std::path::absolute(base.join(value[field].as_str().unwrap_or(fallback))).unwrap()
    };
    let database = resolve("db_dir", "db_storage").canonicalize().unwrap();
    let anchor = resolve("keys_file", "all_keys.json");
    let store = resolve("key_store", "keys.dpapi");
    assert!(database.starts_with(&temp));
    assert!(store
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .starts_with(&temp));
    assert_ne!(store, config);
    assert_ne!(store, anchor);
    let binding = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(normalized(&database), normalized(&anchor))).unwrap())
    );
    let previous = if store.exists() {
        let bytes = fs::read(&store).unwrap();
        assert!(bytes.starts_with(b"WXKEYS\0\x01"));
        let plain = dpapi::transform(&bytes[8..], true).unwrap();
        let previous: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(previous["account"], binding);
        assert!(
            previous["image_key"].is_null()
                || previous["image_key"]["bytes"]
                    .as_array()
                    .is_some_and(|bytes| bytes.len() == 17),
            "fixture cannot reuse an unsupported image record"
        );
        Some(previous)
    } else {
        None
    };
    let mut databases = serde_json::Map::new();
    for (name, encoded) in keys.as_object().unwrap() {
        assert!(name.ends_with(".db") && !name.starts_with('_'));
        let encoded = encoded.as_str().unwrap();
        assert_eq!(encoded.len(), 64);
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&encoded[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        databases.insert(
            name.replace('\\', "/"),
            json!({"bytes":bytes,"verification":verification}),
        );
    }
    let revision = previous
        .as_ref()
        .map_or(1, |record| record["revision"].as_u64().unwrap() + 1);
    let image = image
        .map(|bytes| json!({"bytes":bytes,"verification":verification}))
        .or_else(|| previous.as_ref().map(|record| record["image_key"].clone()));
    let record = json!({"version":1, "account":binding, "revision":revision,
        "account_key":null, "database_keys":databases, "image_key":image});
    let plain = Zeroizing::new(serde_json::to_vec(&record).unwrap());
    let encrypted = dpapi::transform(&plain, false).unwrap();
    let mut bytes = b"WXKEYS\0\x01".to_vec();
    bytes.extend_from_slice(&encrypted);
    fs::write(&store, bytes).unwrap();
    value["key_store"] = json!(store);
    fs::write(config, serde_json::to_vec(&value).unwrap()).unwrap();
}
