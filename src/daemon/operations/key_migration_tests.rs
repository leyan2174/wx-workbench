use super::*;
use serde_json::json;
use std::fs;

fn fixture() -> (tempfile::TempDir, RuntimeContext) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("db_storage/contact/contact.db");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let plain = root.path().join("synthetic.db");
    let db = crate::daemon::query::encrypted_cache::sqlite(&plain);
    db.execute_batch(
        "CREATE TABLE synthetic(value TEXT); INSERT INTO synthetic VALUES('fixture');",
    )
    .unwrap();
    drop(db);
    crate::daemon::query::encrypted_cache::seed(&plain, &source);
    let config = root.path().join("config.json");
    fs::write(
        &config,
        json!({"db_dir":"db_storage", "keys_file":"all_keys.json", "unchanged":{"flag":42}})
            .to_string(),
    )
    .unwrap();
    fs::write(
        root.path().join("all_keys.json"),
        json!({"_db_dir":"db_storage", "contact/contact.db":{"enc_key":"11".repeat(32)}})
            .to_string(),
    )
    .unwrap();
    let runtime = RuntimeContext::from_config(
        config.clone(),
        crate::config::load_config_at(&config).unwrap(),
        root.path().join("home"),
    )
    .unwrap();
    (root, runtime)
}

fn reload(runtime: &RuntimeContext) -> RuntimeContext {
    RuntimeContext::from_config(
        runtime.config_path.clone(),
        crate::config::load_config_at(&runtime.config_path).unwrap(),
        runtime.root.clone(),
    )
    .unwrap()
}

#[test]
fn real_verified_import_is_repeatable_and_preserves_runtime_and_unrelated_config() {
    let (_root, runtime) = fixture();
    let legacy = fs::read(&runtime.config.keys_file).unwrap();
    let args = Args {
        allow_unverified: false,
        cleanup_legacy: true,
    };
    let report = migrate(&runtime, &args).unwrap();
    assert_eq!(report["verified_materials"], 1);
    assert_eq!(report["unverified_materials"], 0);
    let next = reload(&runtime);
    assert_eq!(runtime.id, next.id);
    assert_eq!(runtime.directory, next.directory);
    let store = Store::for_runtime(&next).unwrap();
    let ciphertext = fs::read(store.path()).unwrap();
    assert_eq!(
        store.load().unwrap().database_keys()["contact/contact.db"],
        "11".repeat(32)
    );
    let second = migrate(&next, &args).unwrap();
    assert_eq!(second["revision"], report["revision"]);
    assert_eq!(fs::read(store.path()).unwrap(), ciphertext);
    assert_eq!(fs::read(&next.config.keys_file).unwrap(), legacy);
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(&next.config_path).unwrap()).unwrap();
    assert_eq!(config["unchanged"]["flag"], 42);
    assert!(!report.to_string().contains(&"11".repeat(32)));
}

#[test]
fn invalid_material_duplicate_fields_and_corrupt_store_do_not_switch_config() {
    for mode in ["invalid_key", "duplicate", "corrupt_store"] {
        let (root, runtime) = fixture();
        let config_before = fs::read(&runtime.config_path).unwrap();
        match mode {
            "invalid_key" => fs::write(
                &runtime.config.keys_file,
                json!({"contact/contact.db":"22".repeat(32)}).to_string(),
            )
            .unwrap(),
            "duplicate" => fs::write(
                &runtime.config.keys_file,
                format!(
                    "{{\"contact/contact.db\":{{\"enc_key\":\"{}\",\"enc_key\":\"{}\"}}}}",
                    "11".repeat(32),
                    "11".repeat(32)
                ),
            )
            .unwrap(),
            _ => fs::write(
                root.path().join("keys.dpapi"),
                b"corrupted synthetic ciphertext",
            )
            .unwrap(),
        }
        assert!(migrate(
            &runtime,
            &Args {
                allow_unverified: true,
                cleanup_legacy: true
            }
        )
        .is_err());
        assert_eq!(fs::read(&runtime.config_path).unwrap(), config_before);
    }
}

#[test]
fn interrupted_reference_publication_reuses_ciphertext_on_retry() {
    let (root, runtime) = fixture();
    let original = fs::read(&runtime.config_path).unwrap();
    let pin = crate::service::config_pin::ConfigPin::new(&runtime).unwrap();
    let args = Args {
        allow_unverified: false,
        cleanup_legacy: true,
    };
    assert!(migrate(&runtime, &args).is_err());
    assert_eq!(fs::read(&runtime.config_path).unwrap(), original);
    let ciphertext = fs::read(root.path().join("keys.dpapi")).unwrap();
    assert!(!ciphertext
        .windows(64)
        .any(|bytes| bytes == "11".repeat(32).as_bytes()));
    drop(pin);
    let report = migrate(&runtime, &args).unwrap();
    assert_eq!(report["revision"], 1);
    assert_eq!(
        fs::read(root.path().join("keys.dpapi")).unwrap(),
        ciphertext
    );
    assert_eq!(reload(&runtime).id, runtime.id);
}

#[tokio::test]
async fn migration_preserves_real_task_history_without_resuming_queued_work() {
    use crate::service::protocol::{Call, Kind, Options, SettingsInput, Submission};
    use std::sync::Arc;
    let (_root, runtime) = fixture();
    fs::create_dir_all(&runtime.directory).unwrap();
    let (service, queue) = crate::daemon::tasks::Service::new(
        runtime.clone(),
        Arc::new(crate::daemon::query_state::QueryState::new(runtime.clone())),
    )
    .unwrap();
    service
        .dispatch(Call::Configure {
            settings: SettingsInput::default(),
        })
        .await
        .unwrap();
    let task = service
        .dispatch(Call::Submit {
            idempotency_key: "d".repeat(64),
            task: Submission {
                kind: Kind::WechatDecrypt,
                options: Options::default(),
            },
        })
        .await
        .unwrap();
    let id = task["id"].as_str().unwrap().to_owned();
    assert_eq!(task["status"], "queued");
    drop(queue);
    drop(service);
    migrate(
        &runtime,
        &Args {
            allow_unverified: false,
            cleanup_legacy: true,
        },
    )
    .unwrap();
    let next = reload(&runtime);
    assert_eq!(next.directory, runtime.directory);
    let (restored, mut queue) = crate::daemon::tasks::Service::new(
        next.clone(),
        Arc::new(crate::daemon::query_state::QueryState::new(next)),
    )
    .unwrap();
    let task = restored
        .dispatch(Call::Get { id: id.clone() })
        .await
        .unwrap();
    assert_eq!(task["id"], id);
    assert_eq!(task["status"], "interrupted");
    assert!(matches!(
        queue.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[test]
fn account_bound_missing_database_requires_explicit_unverified_import() {
    let (root, runtime) = fixture();
    fs::remove_file(root.path().join("db_storage/contact/contact.db")).unwrap();
    assert!(migrate(
        &runtime,
        &Args {
            allow_unverified: false,
            cleanup_legacy: false
        }
    )
    .is_err());
    let report = migrate(
        &runtime,
        &Args {
            allow_unverified: true,
            cleanup_legacy: false,
        },
    )
    .unwrap();
    assert_eq!(report["verified_materials"], 0);
    assert_eq!(report["unverified_materials"], 1);
}
