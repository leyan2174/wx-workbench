//! Synthetic workers only: no acquisition operation is ever executed.
use crate::{
    daemon::{query_state::QueryState, worker_keys::Broker},
    key_store::Store,
    runtime::RuntimeContext,
    service::{
        client,
        operations::Operation,
        plan::Step,
        protocol::{Call, ServiceError, MAX_REQUEST_BYTES},
        transport,
        worker_keys::{
            Access, DatabaseReadRequest, ImageReadRequest, Input, MaterialChange, RevisionRequest,
            Secret, UpdateRequest,
        },
    },
    windows_process::managed::{Job, SUSPENDED_NO_WINDOW},
};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Child, sync::watch};
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(15);

fn runtime(root: &Path) -> RuntimeContext {
    let path = root.join("config.json");
    fs::create_dir_all(root.join("db_storage")).unwrap();
    fs::write(
        &path,
        json!({"db_dir":"db_storage", "keys_file":"identity-anchor.json",
            "key_store":"keys.dpapi", "decrypted_dir":"decrypted",
            "wechat_process":"Weixin.exe"})
        .to_string(),
    )
    .unwrap();
    let runtime = RuntimeContext::from_config(
        path.clone(),
        crate::config::load_config_at(&path).unwrap(),
        root.join("home"),
    )
    .unwrap();
    fs::create_dir_all(&runtime.directory).unwrap();
    runtime
}

fn database_operation() -> Operation {
    Operation::DatabaseKeys {
        args: crate::service::operation_requests::database_keys::Args {
            authorize_memory_scan: true,
        },
    }
}

fn memory_initialization() -> Operation {
    Operation::Initialize {
        force: false,
        db_dir_override: None,
        provider: crate::service::operation_requests::key_provider::KeyProvider::Memory,
        restart: false,
        executable: None,
        timeout: 1,
    }
}

fn initialization_with(
    force: bool,
    provider: crate::service::operation_requests::key_provider::KeyProvider,
) -> Operation {
    Operation::Initialize {
        force,
        db_dir_override: None,
        provider,
        restart: false,
        executable: None,
        timeout: 1,
    }
}

fn initialization_with_override(path: String) -> Operation {
    Operation::Initialize {
        force: false,
        db_dir_override: Some(path),
        provider: crate::service::operation_requests::key_provider::KeyProvider::Memory,
        restart: false,
        executable: None,
        timeout: 1,
    }
}

#[tokio::test]
async fn initialization_grant_is_bound_to_complete_memory_configuration() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "init");
    let (access, _registration) = broker
        .register(&worker.child, &memory_initialization())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0);
    let initialization = access.initialization.unwrap();
    assert!(!initialization.has_database_keys);
    assert!(initialization.account.is_none());
    assert!(access.image.is_none());
    fs::create_dir(root.path().join("other-db")).unwrap();
    for operation in [
        initialization_with(
            true,
            crate::service::operation_requests::key_provider::KeyProvider::Memory,
        ),
        initialization_with(
            false,
            crate::service::operation_requests::key_provider::KeyProvider::Saved,
        ),
    ] {
        let (access, _registration) = broker
            .register(&worker.child, &operation)
            .await
            .unwrap()
            .unwrap();
        assert!(access.initialization.unwrap().account.is_none());
    }
    assert!(broker
        .register(
            &worker.child,
            &initialization_with(
                false,
                crate::service::operation_requests::key_provider::KeyProvider::Account,
            ),
        )
        .await
        .unwrap()
        .is_none());
    assert!(broker
        .register(
            &worker.child,
            &initialization_with_override(
                root.path().join("other-db").to_string_lossy().into_owned()
            ),
        )
        .await
        .is_err());
    worker.finish().await;
}

#[tokio::test]
async fn saved_initialization_receives_only_redacted_account_material() {
    use crate::key_store::{Update, Verification};

    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = Store::for_runtime(&runtime).unwrap();
    store
        .update(
            Some(0),
            &[Update::Account(&[0x17; 32], Verification::Unverified)],
        )
        .unwrap();
    let unverified_broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "unverified-saved-init");
    let (access, registration) = unverified_broker
        .register(
            &worker.child,
            &initialization_with(
                true,
                crate::service::operation_requests::key_provider::KeyProvider::Saved,
            ),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(access.initialization.unwrap().account.is_none());
    drop(registration);
    worker.finish().await;

    store
        .update(
            Some(1),
            &[Update::Account(&[0x17; 32], Verification::Verified)],
        )
        .unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "saved-init");
    let (access, _registration) = broker
        .register(
            &worker.child,
            &initialization_with(
                true,
                crate::service::operation_requests::key_provider::KeyProvider::Saved,
            ),
        )
        .await
        .unwrap()
        .unwrap();
    let debug = format!("{access:?}");
    // 运行路径和进程标识可能包含任意数字；精确检查秘密字段的脱敏表示。
    assert!(debug.contains("account: Some(AccountMaterial([REDACTED]))"));
    let seed = access.initialization.unwrap();
    assert!(!seed.has_database_keys);
    assert_eq!(
        format!("{:?}", seed.account.as_ref().unwrap()),
        "AccountMaterial([REDACTED])"
    );
    assert_eq!(seed.account.unwrap().as_bytes(), &[0x17; 32]);
    worker.finish().await;
}

#[tokio::test]
async fn authorized_account_capture_commits_account_and_databases_in_one_revision() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let operation = Operation::Initialize {
        force: true,
        db_dir_override: None,
        provider: crate::service::operation_requests::key_provider::KeyProvider::Account,
        restart: true,
        executable: None,
        timeout: 300,
    };
    let (access, _registration) = broker
        .register(&worker.child, &operation)
        .await
        .unwrap()
        .unwrap();
    assert!(access.initialization.is_none());
    let revision = broker
        .update(
            worker.pid,
            UpdateRequest {
                capability: access.capability,
                expected_revision: access.revision,
                changes: vec![
                    MaterialChange::Account(vec![0x17; 32]),
                    MaterialChange::Databases(HashMap::from([(
                        "contact/contact.db".into(),
                        "31".repeat(32),
                    )])),
                ],
            },
        )
        .await
        .unwrap();
    assert_eq!(revision, 1);
    let saved = Store::for_runtime(&runtime).unwrap().load().unwrap();
    assert_eq!(saved.verified_account_key(), Some([0x17; 32].as_slice()));
    assert_eq!(
        saved.database_keys().get("contact/contact.db"),
        Some(&"31".repeat(32))
    );
    worker.finish().await;
}

#[test]
fn daemon_committed_keys_do_not_require_a_second_snapshot_reload() {
    assert!(!database_operation().requires_snapshot_reload());
}

fn request(access: &Access, marker: u8) -> UpdateRequest {
    UpdateRequest {
        capability: access.capability.clone(),
        expected_revision: access.revision,
        changes: vec![MaterialChange::Databases(HashMap::from([(
            "contact/contact.db".into(),
            format!("{marker:02x}").repeat(32),
        )]))],
    }
}

fn database_request(reverse: bool) -> UpdateRequest {
    let entries = [
        ("contact/contact.db", "11".repeat(32)),
        ("message/message_0.db", "22".repeat(32)),
        ("message/message_1.db", "33".repeat(32)),
    ];
    let mut keys = HashMap::new();
    for index in if reverse { [2, 1, 0] } else { [0, 1, 2] } {
        keys.insert(entries[index].0.to_owned(), entries[index].1.clone());
    }
    UpdateRequest {
        capability: Secret::new("synthetic-signature-only"),
        expected_revision: 0,
        changes: vec![MaterialChange::Databases(keys)],
    }
}

#[test]
fn signature_sorts_database_names_but_distinguishes_revision_and_material() {
    let original = database_request(false);
    let signature = super::update_signature(&original);
    assert!(signature == super::update_signature(&database_request(true)));
    // Explicit JSON member order, not serde_json::Value's sorted object representation.
    let first = Zeroizing::new(format!(
        r#"{{"contact/contact.db":"{}","message/message_0.db":"{}","message/message_1.db":"{}"}}"#,
        "11".repeat(32),
        "22".repeat(32),
        "33".repeat(32),
    ));
    let reversed = Zeroizing::new(format!(
        r#"{{"message/message_1.db":"{}","message/message_0.db":"{}","contact/contact.db":"{}"}}"#,
        "33".repeat(32),
        "22".repeat(32),
        "11".repeat(32),
    ));
    for wire in [&first, &reversed] {
        let mut decoded = database_request(false);
        decoded.changes = vec![MaterialChange::Databases(
            serde_json::from_str(wire).unwrap(),
        )];
        assert!(signature == super::update_signature(&decoded));
    }
    let mut revision = original.clone();
    revision.expected_revision += 1;
    assert!(signature != super::update_signature(&revision));
    let mut material = original.clone();
    let MaterialChange::Databases(keys) = &mut material.changes[0] else {
        unreachable!()
    };
    keys.insert("message/message_1.db".into(), "44".repeat(32));
    assert!(signature != super::update_signature(&material));
    let mut renamed = original.clone();
    let MaterialChange::Databases(keys) = &mut renamed.changes[0] else {
        unreachable!()
    };
    let key = keys.remove("message/message_1.db").unwrap();
    keys.insert("message/message_2.db".into(), key);
    assert!(signature != super::update_signature(&renamed));
}

struct Worker {
    child: Child,
    job: Job,
    pid: u32,
}

impl Worker {
    fn spawn(root: &Path, mode: &str) -> Self {
        let module = module_path!().split_once("::").unwrap().1;
        let helper = format!("{module}::worker_process");
        let job = Job::new().unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &helper, "--ignored", "--nocapture"])
            .env_clear()
            .env("WX_BROKER_TEST_MODE", mode)
            .env("HOME", root)
            .env("USERPROFILE", root)
            .env("TEMP", root)
            .env("TMP", root)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(SUSPENDED_NO_WINDOW);
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let child = command.spawn().unwrap();
        job.attach(&child).unwrap();
        let pid = child.id().unwrap();
        Self { child, job, pid }
    }

    async fn finish(&mut self) {
        self.job.start_terminate().unwrap();
        tokio::time::timeout(DEADLINE, self.child.wait())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(DEADLINE, async {
            while !self.job.is_empty().unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn send(&mut self, access: &Access) {
        self.send_input(access).await;
    }

    async fn send_input(&mut self, input: &impl serde::Serialize) {
        let bytes = Zeroizing::new(serde_json::to_vec(input).unwrap());
        let mut stdin = self.child.stdin.take().unwrap();
        tokio::time::timeout(DEADLINE, async {
            transport::write_frame(&mut stdin, &bytes).await.unwrap();
            stdin.shutdown().await.unwrap();
        })
        .await
        .unwrap();
    }
}

#[test]
#[ignore = "Synthetic supervised worker, launched by worker_keys tests only"]
fn worker_process() {
    let Ok(mode) = std::env::var("WX_BROKER_TEST_MODE") else {
        return;
    };
    if mode == "hold" {
        // Job ownership normally terminates us; this bounds orphan fixture lifetime.
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    assert!(matches!(
        mode.as_str(),
        "pipe" | "pipe-step" | "pipe-monitor" | "pipe-read" | "pipe-image"
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let bytes = Zeroizing::new(
            transport::read_frame(&mut tokio::io::stdin(), MAX_REQUEST_BYTES)
                .await
                .unwrap(),
        );
        if mode == "pipe-read" {
            let input: Input<Step> = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(input.operation, Step::WechatDecrypt { .. }));
            let access = input.access.unwrap();
            let _guard = crate::service::worker_keys::install(Some(access.clone())).unwrap();
            let keys = crate::service::worker_keys::database_keys(&access.runtime)
                .unwrap()
                .unwrap();
            assert_eq!(keys.0.len(), 4096);
            assert_eq!(
                keys.0.get("message/message_4095.db"),
                Some(&"ab".repeat(32))
            );
            return;
        }
        if mode == "pipe-image" {
            let input: Input<Operation> = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(input.operation, Operation::SnsTimeline { .. }));
            let access = input.access.unwrap();
            assert_eq!(access.revision, 0);
            assert!(access.image.is_none());
            let _guard = crate::service::worker_keys::install(Some(access.clone())).unwrap();
            let material = crate::service::worker_keys::image_material(&access.runtime)
                .unwrap()
                .unwrap();
            assert_eq!(material.aes, SYNTHETIC_AES);
            assert_eq!(material.xor, SYNTHETIC_XOR);
            crate::service::worker_keys::verify_image_revision(&access.runtime).unwrap();
            return;
        }
        if mode == "pipe-monitor" {
            let input: Input<Operation> = serde_json::from_slice(&bytes).unwrap();
            assert!(
                matches!(input.operation, Operation::ImageKeyMonitor { ref args } if args.no_save)
            );
            let access = input.access.unwrap();
            assert_image_access(&access);
            let response = client::request_bound(
                &access.runtime,
                Call::WorkerKeyRevision {
                    request: revision_request(&access),
                },
                &access.parent,
            )
            .await
            .unwrap();
            assert_eq!(response, json!({"verified": true}));
            let error = client::request_bound(
                &access.runtime,
                Call::WorkerKeys {
                    request: image_request(&access),
                },
                &access.parent,
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.downcast_ref::<ServiceError>().unwrap().code,
                "unauthorized"
            );
            return;
        }
        let access: Access = if mode == "pipe-step" {
            let input: Input<Step> = serde_json::from_slice(&bytes).unwrap();
            let access = input.access.unwrap();
            match input.operation {
                Step::WechatKeys {
                    config,
                    authorize_memory_scan: true,
                } => {
                    assert_eq!(config, access.runtime.config_path);
                }
                _ => panic!("Unexpected synthetic step"),
            }
            // Never execute the step: only submit artificial material over its grant.
            for forbidden in [
                MaterialChange::Account(vec![0x11; 32]),
                MaterialChange::Image {
                    aes: [0x11; 16],
                    xor: 0x88,
                },
            ] {
                let mut update = request(&access, 0x41);
                update.changes.push(forbidden);
                let error = client::request_bound(
                    &access.runtime,
                    Call::WorkerKeys { request: update },
                    &access.parent,
                )
                .await
                .unwrap_err();
                assert_eq!(
                    error.downcast_ref::<ServiceError>().unwrap().code,
                    "unauthorized"
                );
                assert!(!access.runtime.config.key_store.as_ref().unwrap().exists());
            }
            access
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        // Two real RPCs exercise the completed-request replay path over the wire.
        for _ in 0..2 {
            let response = client::request_bound(
                &access.runtime,
                Call::WorkerKeys {
                    request: request(&access, 0x41),
                },
                &access.parent,
            )
            .await
            .unwrap();
            assert_eq!(response, json!({"revision": access.revision + 1}));
        }
    });
}

fn broker(runtime: &RuntimeContext) -> Arc<Broker> {
    Broker::new(runtime.clone(), Arc::new(QueryState::new(runtime.clone())))
}

#[tokio::test]
async fn valid_write_persists_revision_and_identical_retry_is_byte_for_byte_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, _registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0);
    let revision = broker
        .update(worker.pid, request(&access, 0x41))
        .await
        .unwrap();
    assert_eq!(revision, 1);
    let store = Store::for_runtime(&runtime).unwrap();
    let disk = store.load().unwrap();
    assert_eq!(disk.revision(), revision);
    assert!(disk.account_key().is_none());
    assert!(disk.database_keys().get("contact/contact.db") == Some(&"41".repeat(32)));
    let bytes = fs::read(store.path()).unwrap();
    assert_eq!(
        broker
            .update(worker.pid, request(&access, 0x41))
            .await
            .unwrap(),
        revision
    );
    assert!(fs::read(store.path()).unwrap() == bytes);
    let advanced = Access {
        revision,
        ..access.clone()
    };
    assert_eq!(
        broker
            .update(worker.pid, request(&advanced, 0x42))
            .await
            .unwrap(),
        2
    );
    assert_eq!(store.load().unwrap().revision(), 2);
    worker.finish().await;
}

#[tokio::test]
async fn reordered_database_retry_returns_original_revision_without_rewriting_ciphertext() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, _registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    let mut original = database_request(false);
    original.capability = access.capability.clone();
    original.expected_revision = access.revision;
    assert_eq!(broker.update(worker.pid, original).await.unwrap(), 1);
    let path = runtime.config.key_store.as_ref().unwrap();
    let before = fs::read(path).unwrap();
    // Fresh hash seeds and opposite insertion order on every retry.
    for _ in 0..8 {
        let mut retry = database_request(true);
        retry.capability = access.capability.clone();
        retry.expected_revision = access.revision;
        assert_eq!(broker.update(worker.pid, retry).await.unwrap(), 1);
        assert!(fs::read(path).unwrap() == before);
    }
    assert_eq!(
        Store::for_runtime(&runtime)
            .unwrap()
            .load()
            .unwrap()
            .revision(),
        1
    );
    worker.finish().await;
}

#[tokio::test]
async fn wrong_pid_permissions_and_revoked_capability_cannot_mutate_store() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        broker
            .update(std::process::id(), request(&access, 0x41))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    for change in [
        MaterialChange::Image {
            aes: [0x11; 16],
            xor: 0x88,
        },
        MaterialChange::Account(vec![0x11; 32]),
    ] {
        let mut forbidden = request(&access, 0x41);
        // A permitted database change must not be partially committed.
        forbidden.changes.push(change);
        assert_eq!(
            broker.update(worker.pid, forbidden).await.unwrap_err().code,
            "unauthorized"
        );
        assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    }
    broker
        .update(worker.pid, request(&access, 0x41))
        .await
        .unwrap();
    let bytes = fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap();
    drop(registration);
    // Revocation must reject even an exact replay of an already completed request.
    assert_eq!(
        broker
            .update(worker.pid, request(&access, 0x41))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap() == bytes);
    worker.finish().await;
}

#[tokio::test]
async fn exited_process_handle_and_closed_broker_reject_previously_valid_grants() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, _registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    worker.finish().await;
    assert_eq!(
        broker
            .update(worker.pid, request(&access, 0x41))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    let mut live = Worker::spawn(root.path(), "hold");
    let (access, _live_registration) = broker
        .register(&live.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    broker.close();
    assert_eq!(
        broker
            .update(live.pid, request(&access, 0x41))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(broker
        .register(&live.child, &database_operation())
        .await
        .is_err());
    assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    live.finish().await;
}

#[tokio::test]
async fn concurrent_grants_with_the_same_revision_have_exactly_one_committed_winner() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut first = Worker::spawn(root.path(), "hold");
    let mut second = Worker::spawn(root.path(), "hold");
    let (a, _a_registration) = broker
        .register(&first.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    let (b, _b_registration) = broker
        .register(&second.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.revision, b.revision);
    let (left, right) = tokio::join!(
        broker.update(first.pid, request(&a, 0x41)),
        broker.update(second.pid, request(&b, 0x42)),
    );
    let marker = match (left, right) {
        (Ok(1), Err(error)) => {
            assert_eq!(error.code, "conflict");
            0x41
        }
        (Err(error), Ok(1)) => {
            assert_eq!(error.code, "conflict");
            0x42
        }
        _ => panic!("Expected one committed write and one revision conflict"),
    };
    let disk = Store::for_runtime(&runtime).unwrap().load().unwrap();
    assert_eq!(disk.revision(), 1);
    assert!(disk.account_key().is_none());
    assert!(
        disk.database_keys().get("contact/contact.db") == Some(&format!("{marker:02x}").repeat(32))
    );
    first.finish().await;
    second.finish().await;
}

#[tokio::test]
async fn cancelling_accepted_waiter_preserves_commit_and_idempotency_record() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (mut access, _registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        broker
            .update(worker.pid, request(&access, 0x41))
            .await
            .unwrap(),
        1
    );
    access.revision = 1;
    let update = request(&access, 0x42);
    let signature = super::update_signature(&update);
    let grant = broker
        .grants
        .lock()
        .unwrap()
        .get(&super::digest(access.capability.as_str().as_bytes()))
        .unwrap()
        .clone();
    let lease = broker.query.key_snapshot().await.unwrap();
    let before = fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap();
    let writer = broker.clone();
    let pending = update.clone();
    let pid = worker.pid;
    let waiter = tokio::spawn(async move { writer.update(pid, pending).await });
    // No other contender owns this grant. Its lock is held across the transaction,
    // which the retained key lease prevents from completing.
    tokio::time::timeout(DEADLINE, async {
        loop {
            match grant.try_lock() {
                Ok(guard) => drop(guard),
                Err(_) => break,
            }
            assert!(
                !waiter.is_finished(),
                "Update ended before acquiring its grant"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap() == before);
    drop(lease);
    {
        let completed = tokio::time::timeout(DEADLINE, grant.lock()).await.unwrap();
        assert_eq!(completed.revision, 2);
        assert!(completed.completed == Some((signature, 2)));
    }
    let store = Store::for_runtime(&runtime).unwrap();
    let disk = store.load().unwrap();
    assert_eq!(disk.revision(), 2);
    assert!(disk.database_keys().get("contact/contact.db") == Some(&"42".repeat(32)));
    let committed = fs::read(store.path()).unwrap();
    assert_eq!(broker.update(worker.pid, update).await.unwrap(), 2);
    assert!(fs::read(store.path()).unwrap() == committed);
    worker.finish().await;
}

fn publish_test_identity(runtime: &RuntimeContext) {
    let identity = client::current_process_identity().unwrap();
    let bytes = serde_json::to_vec(&json!({
        "pid": identity.pid, "exe": std::env::current_exe().unwrap(),
        "created": identity.created,
        "runtime_id": runtime.id,
    }))
    .unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(runtime.pid_path())
        .unwrap();
    crate::private_file::restrict(&file).unwrap();
    std::io::Write::write_all(&mut &file, &bytes).unwrap();
}

#[tokio::test]
async fn named_pipe_authenticates_real_worker_pid_and_commits_private_stdin_material() {
    named_pipe_write(false).await;
}

#[tokio::test]
async fn registered_step_writes_databases_over_named_pipe_but_rejects_account_and_image() {
    named_pipe_write(true).await;
}

#[tokio::test]
async fn register_step_requires_matching_config_and_explicit_supported_write_consent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let before = fs::read(&runtime.config_path).unwrap();
    let other = root.path().join("other-config.json");
    fs::write(&other, &before).unwrap();
    assert!(broker
        .register_step(
            &worker.child,
            &Step::WechatKeys {
                config: other.clone(),
                authorize_memory_scan: true,
            }
        )
        .await
        .is_err());
    for step in [
        Step::WechatKeys {
            config: runtime.config_path.clone(),
            authorize_memory_scan: false,
        },
        Step::ImageKey {
            config: runtime.config_path.clone(),
            authorize_memory_scan: false,
            timeout: 1,
            max_mib: 1,
        },
    ] {
        assert!(broker
            .register_step(&worker.child, &step)
            .await
            .unwrap()
            .is_none());
    }
    assert!(broker.grants.lock().unwrap().is_empty());
    assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    assert!(!root.path().join("output").exists());
    assert!(fs::read(&runtime.config_path).unwrap() == before);
    assert!(fs::read(other).unwrap() == before);
    worker.finish().await;
}

#[tokio::test]
async fn decode_images_step_receives_only_lazy_image_read_access() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let before = fs::read(store.path()).unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let step = Step::DecodeImages {
        config: runtime.config_path.clone(),
        output: root.path().join("output"),
    };
    let (access, _registration) = broker
        .register_step(&worker.child, &step)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0, "image material must be loaded lazily");
    assert!(access.image.is_none());
    let snapshot = broker
        .read_image(
            worker.pid,
            ImageReadRequest {
                capability: access.capability.clone(),
            },
        )
        .await
        .unwrap();
    let material = snapshot.material.unwrap();
    assert_eq!(material.aes, SYNTHETIC_AES);
    assert_eq!(material.xor, SYNTHETIC_XOR);
    assert_eq!(
        broker
            .read_databases(
                worker.pid,
                DatabaseReadRequest {
                    capability: access.capability.clone(),
                },
            )
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        broker
            .update(worker.pid, image_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(fs::read(store.path()).unwrap(), before);
    assert!(!root.path().join("output").exists());
    worker.finish().await;
}

#[tokio::test]
async fn image_publication_operations_receive_only_lazy_image_read_access() {
    use crate::service::operations::ToolkitOperation;

    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let before = fs::read(store.path()).unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let operations = [
        ToolkitOperation::DecodeImages {
            attach_dir: None,
            decoded_dir: None,
            aes_key: None,
            xor_key: None,
            force: false,
        },
        ToolkitOperation::DecodeImage {
            dat_file: "synthetic.dat".into(),
            output_file: None,
        },
        ToolkitOperation::BatchDecryptImages {
            input_dir: "synthetic-input".into(),
            output_dir: None,
        },
    ];
    for operation in operations {
        let (access, _registration) = broker
            .register(&worker.child, &Operation::Toolkit { operation })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(access.revision, 0);
        assert!(access.image.is_none());
        let snapshot = broker
            .read_image(
                worker.pid,
                ImageReadRequest {
                    capability: access.capability.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(snapshot.material.unwrap().aes, SYNTHETIC_AES);
        assert_eq!(
            broker
                .read_databases(
                    worker.pid,
                    DatabaseReadRequest {
                        capability: access.capability.clone(),
                    },
                )
                .await
                .unwrap_err()
                .code,
            "unauthorized"
        );
        assert_eq!(
            broker
                .update(worker.pid, image_request(&access))
                .await
                .unwrap_err()
                .code,
            "unauthorized"
        );
    }
    assert_eq!(fs::read(store.path()).unwrap(), before);
    worker.finish().await;
}

#[tokio::test]
async fn decrypt_step_receives_read_only_database_material_from_daemon_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut writer = Worker::spawn(root.path(), "hold");
    let (write_access, _registration) = broker
        .register(&writer.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    broker
        .update(writer.pid, request(&write_access, 0x41))
        .await
        .unwrap();
    writer.finish().await;

    let mut decryptor = Worker::spawn(root.path(), "hold");
    let (access, _registration) = broker
        .register_step(
            &decryptor.child,
            &Step::WechatDecrypt {
                config: runtime.config_path.clone(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0, "read-only registration must stay lazy");
    let snapshot = broker
        .read_databases(
            decryptor.pid,
            DatabaseReadRequest {
                capability: access.capability.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        snapshot.databases.unwrap().0,
        HashMap::from([("contact/contact.db".into(), "41".repeat(32))])
    );
    assert!(access.image.is_none());
    assert!(access.initialization.is_none());
    assert!(broker
        .update(decryptor.pid, request(&access, 0x42))
        .await
        .is_err());
    let (foreground, _foreground_registration) = broker
        .register(
            &decryptor.child,
            &Operation::Toolkit {
                operation: crate::service::operations::ToolkitOperation::Decrypt {
                    incremental: false,
                    dry_run: false,
                },
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        foreground.revision, 0,
        "foreground read registration must stay lazy"
    );
    let foreground_snapshot = broker
        .read_databases(
            decryptor.pid,
            DatabaseReadRequest {
                capability: foreground.capability.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        foreground_snapshot.databases.unwrap().0,
        HashMap::from([("contact/contact.db".into(), "41".repeat(32))])
    );
    assert!(foreground.image.is_none());
    assert!(foreground.initialization.is_none());
    assert_eq!(
        broker
            .update(decryptor.pid, request(&foreground, 0x42))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    decryptor.finish().await;
}

#[tokio::test]
async fn lazy_database_read_rejects_corrupt_store_wrong_pid_revocation_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    fs::write(
        runtime.config.key_store.as_ref().unwrap(),
        b"synthetic-corrupt-key-store",
    )
    .unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, registration) = broker
        .register_step(
            &worker.child,
            &Step::WechatDecrypt {
                config: runtime.config_path.clone(),
            },
        )
        .await
        .unwrap()
        .expect("read-only grant");
    assert_eq!(access.revision, 0, "registration must not read the store");
    let request = || DatabaseReadRequest {
        capability: access.capability.clone(),
    };

    assert_eq!(
        broker
            .read_databases(std::process::id(), request())
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        broker
            .read_databases(worker.pid, request())
            .await
            .unwrap_err()
            .code,
        "key_read_failed"
    );

    drop(registration);
    assert_eq!(
        broker
            .read_databases(worker.pid, request())
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );

    let (closing_access, _registration) = broker
        .register_step(
            &worker.child,
            &Step::WechatDecrypt {
                config: runtime.config_path.clone(),
            },
        )
        .await
        .unwrap()
        .expect("second read-only grant");
    broker.close();
    assert_eq!(
        broker
            .read_databases(
                worker.pid,
                DatabaseReadRequest {
                    capability: closing_access.capability,
                },
            )
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    worker.finish().await;
}

fn image_operation(offline: bool, authorize_memory_scan: bool, no_save: bool) -> Operation {
    Operation::ImageKeys {
        args: crate::service::operation_requests::image_keys::Args {
            sample: Default::default(),
            offline,
            authorize_memory_scan,
            no_save,
            timeout: 1,
            max_mib: 1,
        },
    }
}

fn image_monitor(authorize_memory_scan: bool, no_save: bool) -> Operation {
    Operation::ImageKeyMonitor {
        args: crate::service::operation_requests::image_keys::MonitorArgs {
            sample: Default::default(),
            authorize_memory_scan,
            no_save,
            scan_seconds: 1,
            interval_ms: 100,
            timeout: 1,
            max_mib: 1,
        },
    }
}

#[tokio::test]
async fn image_grants_require_supported_mode_consent_save_and_matching_step_config() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let before = fs::read(&runtime.config_path).unwrap();
    for offline in [false, true] {
        for consent in [false, true] {
            for no_save in [false, true] {
                let grant = broker
                    .register(&worker.child, &image_operation(offline, consent, no_save))
                    .await
                    .unwrap();
                assert_eq!(grant.is_some(), !no_save && (offline != consent));
                drop(grant);
            }
        }
    }
    for consent in [false, true] {
        for no_save in [false, true] {
            let grant = broker
                .register(&worker.child, &image_monitor(consent, no_save))
                .await
                .unwrap();
            assert_eq!(grant.is_some(), consent);
            if let Some((access, _)) = &grant {
                assert_eq!(access.revision, 0);
                assert!(access.image.is_none());
                broker
                    .verify_image_revision(worker.pid, revision_request(access))
                    .await
                    .unwrap();
            }
            drop(grant);
        }
    }
    let other = root.path().join("other-config.json");
    fs::write(&other, &before).unwrap();
    assert!(broker
        .register_step(
            &worker.child,
            &Step::ImageKey {
                config: other.clone(),
                authorize_memory_scan: true,
                timeout: 1,
                max_mib: 1,
            }
        )
        .await
        .is_err());
    for consent in [false, true] {
        let grant = broker
            .register_step(
                &worker.child,
                &Step::ImageKey {
                    config: runtime.config_path.clone(),
                    authorize_memory_scan: consent,
                    timeout: 1,
                    max_mib: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(grant.is_some(), consent);
        drop(grant);
    }
    assert!(broker.grants.lock().unwrap().is_empty());
    assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    assert!(fs::read(&runtime.config_path).unwrap() == before);
    assert!(fs::read(other).unwrap() == before);
    worker.finish().await;
}

const SYNTHETIC_AES: [u8; 16] = *b"syntheticAESkey1";
const SYNTHETIC_XOR: u8 = 0xa7;

fn seed_image(runtime: &RuntimeContext) -> Store {
    let store = Store::for_runtime(runtime).unwrap();
    store
        .update(
            Some(0),
            &[crate::key_store::Update::Image(
                &SYNTHETIC_AES,
                SYNTHETIC_XOR,
                crate::key_store::Verification::Unverified,
            )],
        )
        .unwrap();
    store
}

fn revision_request(access: &Access) -> RevisionRequest {
    RevisionRequest {
        capability: access.capability.clone(),
        expected_revision: access.revision,
    }
}

fn image_request(access: &Access) -> UpdateRequest {
    UpdateRequest {
        capability: access.capability.clone(),
        expected_revision: access.revision,
        changes: vec![MaterialChange::Image {
            aes: SYNTHETIC_AES,
            xor: SYNTHETIC_XOR,
        }],
    }
}

fn assert_image_access(access: &Access) {
    let image = access.image.as_ref().unwrap();
    assert!(image.aes == SYNTHETIC_AES);
    assert!(image.xor == SYNTHETIC_XOR);
    let debug = format!("{access:?}");
    for secret in [
        access.capability.as_str().to_owned(),
        String::from_utf8(SYNTHETIC_AES.to_vec()).unwrap(),
        format!("{SYNTHETIC_AES:?}"),
        format!("xor: {SYNTHETIC_XOR}"),
    ] {
        assert!(!debug.contains(&secret));
    }
}

#[tokio::test]
async fn image_readers_receive_material_without_inheriting_write_access() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    // Snapshot::image_material also exposes unverified candidates for revalidation.
    let store = seed_image(&runtime);
    let before = fs::read(store.path()).unwrap();
    let config_before = fs::read(&runtime.config_path).unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    for operation in [
        database_operation(),
        image_operation(true, false, false),
        image_operation(false, true, false),
        image_monitor(true, false),
        image_monitor(true, true),
        Operation::ExportMessages {
            args: Default::default(),
        },
        Operation::SnsArchive {
            args: Default::default(),
        },
    ] {
        let (access, _registration) = broker
            .register(&worker.child, &operation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(access.revision, 1);
        if matches!(
            operation,
            Operation::ImageKeyMonitor { .. }
                | Operation::ExportMessages { .. }
                | Operation::SnsArchive { .. }
        ) {
            assert_image_access(&access);
            broker
                .verify_image_revision(worker.pid, revision_request(&access))
                .await
                .unwrap();
            if matches!(operation, Operation::ImageKeyMonitor { ref args } if args.no_save) {
                for update in [
                    image_request(&access),
                    request(&access, 0x42),
                    UpdateRequest {
                        capability: access.capability.clone(),
                        expected_revision: access.revision,
                        changes: vec![MaterialChange::Account(vec![0x42; 32])],
                    },
                ] {
                    assert_eq!(
                        broker.update(worker.pid, update).await.unwrap_err().code,
                        "unauthorized"
                    );
                }
            }
            if matches!(operation, Operation::ExportMessages { .. }) {
                assert_eq!(
                    broker
                        .update(worker.pid, image_request(&access))
                        .await
                        .unwrap_err()
                        .code,
                    "unauthorized"
                );
                broker
                    .read_databases(
                        worker.pid,
                        DatabaseReadRequest {
                            capability: access.capability.clone(),
                        },
                    )
                    .await
                    .unwrap();
            }
            if matches!(operation, Operation::SnsArchive { .. }) {
                assert_eq!(
                    broker
                        .update(worker.pid, image_request(&access))
                        .await
                        .unwrap_err()
                        .code,
                    "unauthorized"
                );
                assert_eq!(
                    broker
                        .read_databases(
                            worker.pid,
                            DatabaseReadRequest {
                                capability: access.capability.clone(),
                            },
                        )
                        .await
                        .unwrap_err()
                        .code,
                    "unauthorized"
                );
            }
            let input = Input {
                operation,
                access: Some(access.clone()),
            };
            let debug = format!("{input:?}");
            assert!(!debug.contains(access.capability.as_str()));
            assert!(!debug.contains(&format!("{SYNTHETIC_AES:?}")));
            let bytes = Zeroizing::new(serde_json::to_vec(&input).unwrap());
            let decoded: Input<Operation> = serde_json::from_slice(&bytes).unwrap();
            let decoded = decoded.access.unwrap();
            assert_eq!(decoded.revision, access.revision);
            assert!(decoded.capability.as_str() == access.capability.as_str());
            assert_image_access(&decoded);
        } else {
            assert!(access.image.is_none());
            assert_eq!(
                broker
                    .verify_image_revision(worker.pid, revision_request(&access))
                    .await
                    .unwrap_err()
                    .code,
                "unauthorized"
            );
        }
    }
    for args in [
        crate::service::operation_requests::export_messages::Args {
            dry_run: true,
            ..Default::default()
        },
        crate::service::operation_requests::export_messages::Args {
            no_media: true,
            ..Default::default()
        },
    ] {
        assert!(broker
            .register(&worker.child, &Operation::ExportMessages { args })
            .await
            .unwrap()
            .is_none());
    }
    let (access, _registration) = broker
        .register_step(
            &worker.child,
            &Step::ImageKey {
                config: runtime.config_path.clone(),
                authorize_memory_scan: true,
                timeout: 1,
                max_mib: 1,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(access.image.is_none());
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    let (access, _registration) = broker
        .register_step(
            &worker.child,
            &Step::SnsArchive {
                config: runtime.config_path.clone(),
                output: root.path().join("sns-output"),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_image_access(&access);
    assert_eq!(
        broker
            .update(worker.pid, image_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    let (access, _registration) = broker
        .register_step(
            &worker.child,
            &Step::ExportMessages {
                config: runtime.config_path.clone(),
                output: root.path().join("chat-output"),
                users: Vec::new(),
                formats: Vec::new(),
                include_images: true,
                allow_missing_media: false,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_image_access(&access);
    broker
        .read_databases(
            worker.pid,
            DatabaseReadRequest {
                capability: access.capability.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        broker
            .update(worker.pid, image_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(fs::read(store.path()).unwrap() == before);
    assert!(fs::read(&runtime.config_path).unwrap() == config_before);
    worker.finish().await;
}

#[tokio::test]
async fn image_revision_rejects_wrong_identity_revocation_and_concurrent_commit() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let before = fs::read(store.path()).unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    for (pid, request) in [
        (std::process::id(), revision_request(&access)),
        (
            worker.pid,
            RevisionRequest {
                capability: Secret::new("unknown-synthetic-capability"),
                expected_revision: access.revision,
            },
        ),
    ] {
        assert_eq!(
            broker
                .verify_image_revision(pid, request)
                .await
                .unwrap_err()
                .code,
            "unauthorized"
        );
    }
    let mut wrong_revision = revision_request(&access);
    wrong_revision.expected_revision += 1;
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, wrong_revision)
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    broker
        .verify_image_revision(worker.pid, revision_request(&access))
        .await
        .unwrap();
    drop(registration);
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(fs::read(store.path()).unwrap() == before);

    let (reader, _reader_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    let (writer, _writer_registration) = broker
        .register(&worker.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reader.revision, writer.revision);
    let revision = broker
        .update(worker.pid, request(&writer, 0x43))
        .await
        .unwrap();
    assert_eq!(revision, reader.revision + 1);
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&reader))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let (fresh, _fresh_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.revision, revision);
    assert_image_access(&fresh);
    broker
        .verify_image_revision(worker.pid, revision_request(&fresh))
        .await
        .unwrap();
    worker.finish().await;
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&fresh))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
}

#[tokio::test]
async fn image_revision_uses_query_snapshot_until_explicit_invalidation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (access, _registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    store
        .update(
            Some(access.revision),
            &[crate::key_store::Update::Image(
                &[0x44; 16],
                0x92,
                crate::key_store::Verification::Verified,
            )],
        )
        .unwrap();
    broker
        .verify_image_revision(worker.pid, revision_request(&access))
        .await
        .unwrap();
    let (cached, _cached_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cached.revision, access.revision);
    assert_image_access(&cached);
    broker.query.invalidate_keys().await;
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&access))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let (fresh, _fresh_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.revision, access.revision + 1);
    assert!(fresh.image.as_ref().unwrap().aes == [0x44; 16]);
    assert!(fresh.image.as_ref().unwrap().xor == 0x92);
    broker
        .verify_image_revision(worker.pid, revision_request(&fresh))
        .await
        .unwrap();
    broker.close();
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&fresh))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    worker.finish().await;
}

#[tokio::test]
async fn no_save_monitor_reuses_cached_image_after_ciphertext_corruption() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    let (initial, _initial_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    assert_image_access(&initial);
    let corrupt = b"synthetic-corrupt-ciphertext";
    fs::write(store.path(), corrupt).unwrap();
    assert!(store.load().is_err());
    let (cached, _cached_registration) = broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cached.revision, initial.revision);
    assert_image_access(&cached);
    broker
        .verify_image_revision(worker.pid, revision_request(&cached))
        .await
        .unwrap();
    assert_eq!(
        broker
            .update(worker.pid, image_request(&cached))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert!(fs::read(store.path()).unwrap() == corrupt);
    broker.query.invalidate_keys().await;
    assert_eq!(
        broker
            .verify_image_revision(worker.pid, revision_request(&cached))
            .await
            .unwrap_err()
            .code,
        "key_read_failed"
    );
    assert!(broker
        .register(&worker.child, &image_monitor(true, true))
        .await
        .is_err());
    assert!(fs::read(store.path()).unwrap() == corrupt);
    worker.finish().await;
}

#[tokio::test]
async fn image_broker_save_preserves_other_material_config_and_rejects_stale_writers() {
    use crate::key_store::{Update, Verification};
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let config_before = fs::read(&runtime.config_path).unwrap();
    let store = Store::for_runtime(&runtime).unwrap();
    let database_keys = HashMap::from([("contact/contact.db".into(), "31".repeat(32))]);
    let seeded = store
        .update(
            Some(0),
            &[
                Update::Account(&[0x17; 32], Verification::Verified),
                Update::Databases(&database_keys, Verification::Verified),
            ],
        )
        .unwrap();
    assert_eq!(seeded.revision(), 1);
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "hold");
    for source in 0..4 {
        let step = Step::ImageKey {
            config: runtime.config_path.clone(),
            authorize_memory_scan: true,
            timeout: 1,
            max_mib: 1,
        };
        let operation = match source {
            0 => image_operation(true, false, false),
            1 => image_operation(false, true, false),
            _ => image_monitor(true, false),
        };
        let (access, _registration) = if source == 3 {
            broker.register_step(&worker.child, &step).await
        } else {
            broker.register(&worker.child, &operation).await
        }
        .unwrap()
        .unwrap();
        let (stale, _stale_registration) = if source == 3 {
            broker.register_step(&worker.child, &step).await
        } else {
            broker.register(&worker.child, &operation).await
        }
        .unwrap()
        .unwrap();
        let before = fs::read(store.path()).unwrap();
        let material = crate::attachment::image_key::ImageKeyMaterial {
            aes_key: *b"syntheticAESkey1",
            xor_key: 0xa2 + source as u8,
        };
        let update = UpdateRequest {
            capability: access.capability.clone(),
            expected_revision: access.revision,
            changes: vec![MaterialChange::Image {
                aes: material.aes_key,
                xor: material.xor_key,
            }],
        };
        for forbidden in [
            MaterialChange::Account(vec![0x44; 32]),
            MaterialChange::Databases(HashMap::from([(
                "contact/contact.db".into(),
                "44".repeat(32),
            )])),
        ] {
            let mut mixed = update.clone();
            mixed.changes.push(forbidden);
            assert_eq!(
                broker.update(worker.pid, mixed).await.unwrap_err().code,
                "unauthorized"
            );
            assert!(fs::read(store.path()).unwrap() == before);
        }
        for debug in [
            format!("{material:?}"),
            format!("{:?}", update.changes[0]),
            format!("{update:?}"),
        ] {
            assert!(!debug.contains("synthetic"));
            assert!(!debug.contains(&format!("{:?}", material.aes_key)));
            assert!(!debug.contains(&material.xor_key.to_string()));
            assert!(!debug.contains(access.capability.as_str()));
        }
        let revision = broker.update(worker.pid, update.clone()).await.unwrap();
        assert_eq!(revision, access.revision + 1);
        let disk = store.load().unwrap();
        assert_eq!(disk.revision(), revision);
        assert!(disk.image_key() == Some((material.aes_key, material.xor_key)));
        assert!(disk.account_key() == Some([0x17; 32].as_slice()));
        assert!(disk.database_keys() == database_keys);
        assert!(fs::read(&runtime.config_path).unwrap() == config_before);
        let ciphertext = fs::read(store.path()).unwrap();
        for secret in [
            material.aes_key.to_vec(),
            vec![0x17; 32],
            vec![0x31; 32],
            "31".repeat(32).into_bytes(),
        ] {
            assert!(!ciphertext
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_slice()));
        }
        crate::private_file::assert_private_acl(store.path());
        let old_update = UpdateRequest {
            capability: stale.capability.clone(),
            ..update.clone()
        };
        assert_eq!(
            broker
                .update(worker.pid, old_update)
                .await
                .unwrap_err()
                .code,
            "conflict"
        );
        assert!(fs::read(store.path()).unwrap() == ciphertext);
        assert_eq!(broker.update(worker.pid, update).await.unwrap(), revision);
        assert!(fs::read(store.path()).unwrap() == ciphertext);
        assert_eq!(store.load().unwrap().revision(), revision);
    }
    worker.finish().await;
}

#[tokio::test]
async fn named_pipe_monitor_reads_private_stdin_image_and_receives_only_verification() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let before = fs::read(store.path()).unwrap();
    let config_before = fs::read(&runtime.config_path).unwrap();
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), "pipe-monitor");
    let operation = image_monitor(true, true);
    let (access, _registration) = broker
        .register(&worker.child, &operation)
        .await
        .unwrap()
        .unwrap();
    publish_test_identity(&runtime);
    let handler_broker = broker.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let handler = Arc::new(move |call, peer_pid| {
        let broker = handler_broker.clone();
        let calls = handler_calls.clone();
        async move {
            match call {
                Call::Info {} => Ok(json!({})),
                Call::WorkerKeyRevision { request } => {
                    calls.fetch_add(1, Ordering::SeqCst);
                    broker.verify_image_revision(peer_pid, request).await?;
                    Ok(json!({"verified": true}))
                }
                Call::WorkerKeys { request } => {
                    calls.fetch_add(1, Ordering::SeqCst);
                    broker
                        .update(peer_pid, request)
                        .await
                        .map(|revision| json!({"revision": revision}))
                }
                _ => Err(ServiceError::new(
                    "invalid_request",
                    "Unexpected synthetic call",
                )),
            }
        }
    });
    let (shutdown, receiver) = watch::channel(false);
    let mut serving = tokio::spawn(transport::serve(runtime.clone(), handler, receiver));
    client::wait_ready(&runtime).await.unwrap();
    let mut wrong_parent = access.parent.clone();
    wrong_parent.created ^= 1;
    let error = client::request_bound(
        &runtime,
        Call::WorkerKeyRevision {
            request: revision_request(&access),
        },
        &wrong_parent,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ServiceError>().unwrap().code,
        "unauthorized"
    );
    assert!(error.to_string().contains("no request was sent"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let error = client::request(
        &runtime,
        Call::WorkerKeyRevision {
            request: revision_request(&access),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ServiceError>().unwrap().code,
        "unauthorized"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    worker
        .send_input(&Input {
            operation,
            access: Some(access),
        })
        .await;
    let status = tokio::time::timeout(DEADLINE, worker.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "Synthetic monitor RPC failed; child output intentionally suppressed"
    );
    assert!(fs::read(store.path()).unwrap() == before);
    assert!(fs::read(&runtime.config_path).unwrap() == config_before);
    shutdown.send_replace(true);
    tokio::time::timeout(DEADLINE, &mut serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    worker.finish().await;
}

#[tokio::test]
async fn named_pipe_reads_large_database_snapshot_without_expanding_worker_input() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut writer = Worker::spawn(root.path(), "hold");
    let (write_access, _writer_registration) = broker
        .register(&writer.child, &database_operation())
        .await
        .unwrap()
        .unwrap();
    let database_keys = (0..4096)
        .map(|index| (format!("message/message_{index}.db"), "ab".repeat(32)))
        .collect();
    broker
        .update(
            writer.pid,
            UpdateRequest {
                capability: write_access.capability,
                expected_revision: write_access.revision,
                changes: vec![MaterialChange::Databases(database_keys)],
            },
        )
        .await
        .unwrap();
    writer.finish().await;

    let mut reader = Worker::spawn(root.path(), "pipe-read");
    let step = Step::WechatDecrypt {
        config: runtime.config_path.clone(),
    };
    let (access, _reader_registration) = broker
        .register_step(&reader.child, &step)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0);
    let input = Input {
        operation: &step,
        access: Some(access),
    };
    assert!(
        serde_json::to_vec(&input).unwrap().len() < MAX_REQUEST_BYTES,
        "database keys must not be embedded in worker stdin"
    );

    publish_test_identity(&runtime);
    let handler = Arc::new(|call, _peer_pid| async move {
        match call {
            Call::Info {} => Ok(json!({})),
            _ => Err(ServiceError::new(
                "invalid_request",
                "Unexpected synthetic call",
            )),
        }
    });
    let read_calls = Arc::new(AtomicUsize::new(0));
    let handler_broker = broker.clone();
    let handler_calls = read_calls.clone();
    let database_handler: transport::DatabaseKeyHandler = Arc::new(move |request, peer_pid| {
        let broker = handler_broker.clone();
        let calls = handler_calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            broker.read_databases(peer_pid, request).await
        })
    });
    let (shutdown, receiver) = watch::channel(false);
    let image_handler: transport::ImageKeyHandler =
        Arc::new(|_, _| Box::pin(async { Err(ServiceError::unauthorized()) }));
    let mut serving = tokio::spawn(transport::serve_with_worker_keys(
        runtime.clone(),
        handler,
        transport::WorkerKeyHandlers {
            databases: database_handler,
            image: image_handler,
        },
        receiver,
    ));
    client::wait_ready(&runtime).await.unwrap();
    reader.send_input(&input).await;
    let status = tokio::time::timeout(DEADLINE, reader.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "synthetic database reader failed");
    assert_eq!(read_calls.load(Ordering::SeqCst), 1);
    shutdown.send_replace(true);
    tokio::time::timeout(DEADLINE, &mut serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    reader.finish().await;
}

#[tokio::test]
async fn named_pipe_lazily_reads_image_material_without_write_access() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let store = seed_image(&runtime);
    let store_before = fs::read(store.path()).unwrap();
    let config_before = fs::read(&runtime.config_path).unwrap();
    let broker = broker(&runtime);
    let mut reader = Worker::spawn(root.path(), "pipe-image");
    let operation = Operation::SnsTimeline {
        args: Default::default(),
    };
    let (access, _registration) = broker
        .register(&reader.child, &operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(access.revision, 0);
    assert!(access.image.is_none());
    assert_eq!(
        broker
            .update(reader.pid, image_request(&access))
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        broker
            .read_databases(
                reader.pid,
                DatabaseReadRequest {
                    capability: access.capability.clone(),
                },
            )
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );
    assert_eq!(
        broker
            .read_image(
                reader.pid.wrapping_add(1),
                ImageReadRequest {
                    capability: access.capability.clone(),
                },
            )
            .await
            .unwrap_err()
            .code,
        "unauthorized"
    );

    publish_test_identity(&runtime);
    let database_handler: transport::DatabaseKeyHandler =
        Arc::new(|_, _| Box::pin(async { Err(ServiceError::unauthorized()) }));
    let image_calls = Arc::new(AtomicUsize::new(0));
    let image_broker = broker.clone();
    let handler_calls = image_calls.clone();
    let image_handler: transport::ImageKeyHandler = Arc::new(move |request, peer_pid| {
        let broker = image_broker.clone();
        let calls = handler_calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            broker.read_image(peer_pid, request).await
        })
    });
    let verification_broker = broker.clone();
    let handler_calls = image_calls.clone();
    let handler = Arc::new(move |call, peer_pid| {
        let broker = verification_broker.clone();
        let calls = handler_calls.clone();
        async move {
            match call {
                Call::Info {} => Ok(json!({})),
                Call::WorkerKeyRevision { request } => {
                    calls.fetch_add(1, Ordering::SeqCst);
                    broker.verify_image_revision(peer_pid, request).await?;
                    Ok(json!({"verified": true}))
                }
                _ => Err(ServiceError::new(
                    "invalid_request",
                    "Unexpected synthetic call",
                )),
            }
        }
    });
    let (shutdown, receiver) = watch::channel(false);
    let mut serving = tokio::spawn(transport::serve_with_worker_keys(
        runtime.clone(),
        handler,
        transport::WorkerKeyHandlers {
            databases: database_handler,
            image: image_handler,
        },
        receiver,
    ));
    client::wait_ready(&runtime).await.unwrap();
    reader
        .send_input(&Input {
            operation,
            access: Some(access),
        })
        .await;
    let status = tokio::time::timeout(DEADLINE, reader.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "synthetic image reader failed");
    assert_eq!(image_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read(store.path()).unwrap(), store_before);
    assert_eq!(fs::read(&runtime.config_path).unwrap(), config_before);
    shutdown.send_replace(true);
    tokio::time::timeout(DEADLINE, &mut serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    reader.finish().await;
}

async fn named_pipe_write(step_channel: bool) {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let broker = broker(&runtime);
    let mut worker = Worker::spawn(root.path(), if step_channel { "pipe-step" } else { "pipe" });
    let step = Step::WechatKeys {
        config: runtime.config_path.clone(),
        authorize_memory_scan: true,
    };
    let (access, _registration) = if step_channel {
        broker.register_step(&worker.child, &step).await
    } else {
        broker.register(&worker.child, &database_operation()).await
    }
    .unwrap()
    .unwrap();
    publish_test_identity(&runtime);
    let handler_broker = broker.clone();
    let worker_calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = worker_calls.clone();
    let handler = Arc::new(move |call, peer_pid| {
        let broker = handler_broker.clone();
        let worker_calls = handler_calls.clone();
        async move {
            match call {
                Call::Info {} => Ok(json!({})),
                Call::WorkerKeyRevision { request } => {
                    worker_calls.fetch_add(1, Ordering::SeqCst);
                    broker.verify_image_revision(peer_pid, request).await?;
                    Ok(json!({"verified": true}))
                }
                Call::WorkerKeys { request } => {
                    worker_calls.fetch_add(1, Ordering::SeqCst);
                    // This value comes from GetNamedPipeClientProcessId, not JSON.
                    broker
                        .update(peer_pid, request)
                        .await
                        .map(|revision| json!({"revision":revision}))
                }
                _ => Err(ServiceError::new(
                    "invalid_request",
                    "Unexpected synthetic call",
                )),
            }
        }
    });
    let (shutdown, receiver) = watch::channel(false);
    let mut serving = tokio::spawn(transport::serve(runtime.clone(), handler, receiver));
    client::wait_ready(&runtime).await.unwrap();
    let mut wrong_parent = access.parent.clone();
    wrong_parent.created ^= 1;
    let error = client::request_bound(
        &runtime,
        Call::WorkerKeys {
            request: request(&access, 0x41),
        },
        &wrong_parent,
    )
    .await
    .unwrap_err();
    let denied = error.downcast_ref::<ServiceError>().unwrap();
    assert_eq!(denied.code, "unauthorized");
    assert!(error.to_string().contains("no request was sent"));
    assert_eq!(worker_calls.load(Ordering::SeqCst), 0);
    assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    // A valid capability submitted by the parent over the same real pipe is refused.
    let error = client::request(
        &runtime,
        Call::WorkerKeys {
            request: request(&access, 0x41),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ServiceError>().unwrap().code,
        "unauthorized"
    );
    assert_eq!(worker_calls.load(Ordering::SeqCst), 1);
    assert!(!runtime.config.key_store.as_ref().unwrap().exists());
    if step_channel {
        worker
            .send_input(&Input {
                operation: step,
                access: Some(access.clone()),
            })
            .await;
    } else {
        worker.send(&access).await;
    }
    let status = tokio::time::timeout(DEADLINE, worker.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "Synthetic worker RPC failed; child output intentionally suppressed"
    );
    let store = Store::for_runtime(&runtime).unwrap();
    let disk = store.load().unwrap();
    assert_eq!(disk.revision(), 1);
    assert!(disk.account_key().is_none());
    assert!(disk.database_keys().get("contact/contact.db") == Some(&"41".repeat(32)));
    crate::private_file::assert_private_acl(store.path());
    shutdown.send_replace(true);
    tokio::time::timeout(DEADLINE, &mut serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // The wrong-generation connection sent no WorkerKeys call, including after drain.
    assert_eq!(
        worker_calls.load(Ordering::SeqCst),
        if step_channel { 5 } else { 3 }
    );
    worker.finish().await;
}
