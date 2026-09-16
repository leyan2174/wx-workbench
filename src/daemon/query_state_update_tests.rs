use super::{
    tests::{runtime, seed},
    *,
};
use crate::key_store::{Error, Store, Verification};
use std::{collections::HashMap, fs, future::Future, task::Poll, time::Duration};

const DEADLINE: Duration = Duration::from_secs(10);

fn image_change() -> Vec<KeyChange> {
    vec![KeyChange::Image(
        *b"syntheticAESkey1",
        0x88,
        Verification::Verified,
    )]
}

fn updates(state: &QueryState) -> usize {
    state.key_updates.load(Ordering::Relaxed)
}

fn loads(state: &QueryState) -> usize {
    state.key_loads.load(Ordering::Relaxed)
}

fn probe(
    state: &QueryState,
    panic_after_commit: bool,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (committed, entered) = tokio::sync::oneshot::channel();
    let (release, receiver) = std::sync::mpsc::channel();
    *state.update_probe.lock().unwrap() = Some(KeyUpdateProbe {
        committed,
        release: receiver,
        panic_after_commit,
    });
    (entered, release)
}

#[tokio::test]
async fn account_and_databases_initialize_once_and_publish_returned_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    fs::remove_file(runtime.config.key_store.as_ref().unwrap()).unwrap();
    let state = Arc::new(QueryState::new(runtime.clone()));
    let revision = state
        .update_keys(
            Some(0),
            vec![
                KeyChange::Account(vec![0x44; 32], Verification::Verified),
                KeyChange::Databases(
                    HashMap::from([
                        ("contact/contact.db".into(), "11".repeat(32)),
                        ("message/message_1.db".into(), "22".repeat(32)),
                    ]),
                    Verification::Verified,
                ),
            ],
        )
        .await
        .unwrap();
    assert_eq!(revision, 1);
    assert_eq!(updates(&state), 1);
    assert_eq!(loads(&state), 0);
    assert!(state.snapshot.read().await.get().is_none());
    let (key, query) = tokio::join!(state.key_snapshot(), state.snapshot());
    let key = key.unwrap();
    let query = query.unwrap();
    assert!(std::ptr::eq(key.key_material(), query.key_material()));
    assert_eq!(
        key.key_material().account_key(),
        Some([0x44; 32].as_slice())
    );
    assert_eq!(
        query.names().read().await.msg_db_keys,
        ["message/message_1.db"]
    );
    assert_eq!(loads(&state), 0);
    let disk = Store::for_runtime(&runtime).unwrap().load().unwrap();
    assert_eq!(disk.revision(), revision);
    assert_eq!(disk.database_keys(), key.key_material().database_keys());
    assert_eq!(disk.account_key(), key.key_material().account_key());
}

#[tokio::test]
async fn invalid_material_and_conflicting_revision_do_not_publish_or_change_disk() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let old = Arc::clone(&query.snapshot.key_material);
    drop(query);
    let path = runtime.config.key_store.as_ref().unwrap();
    let disk = fs::read(path).unwrap();
    let mut invalid = image_change();
    invalid.push(KeyChange::Account(vec![0x44; 31], Verification::Verified));
    for (expected, changes, error) in [
        (Some(old.revision()), invalid, Error::Invalid),
        (Some(old.revision() + 1), image_change(), Error::Conflict),
    ] {
        let failure = state.update_keys(expected, changes).await.unwrap_err();
        assert_eq!(failure.downcast_ref::<Error>(), Some(&error));
        assert_eq!(fs::read(path).unwrap(), disk);
        assert!(state.snapshot.read().await.get().is_some());
        let key = state.key_snapshot().await.unwrap();
        assert!(std::ptr::eq(old.as_ref(), key.key_material()));
        assert!(key.key_material().image_key().is_none());
        assert_eq!(loads(&state), 1);
    }
    assert_eq!(updates(&state), 2);
}

#[tokio::test]
async fn atomic_replace_failure_does_not_publish_or_discard_the_old_generation() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let old = Arc::clone(&query.snapshot.key_material);
    drop(query);
    let path = runtime.config.key_store.as_ref().unwrap();
    let disk = fs::read(path).unwrap();
    // Permit reads but deny replacement of the real DPAPI file.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .unwrap();
    let failure = state
        .update_keys(Some(old.revision()), image_change())
        .await
        .unwrap_err();
    assert_eq!(failure.downcast_ref::<Error>(), Some(&Error::Io));
    assert_eq!(updates(&state), 1);
    assert_eq!(fs::read(path).unwrap(), disk);
    assert!(state.snapshot.read().await.get().is_some());
    let key = state.key_snapshot().await.unwrap();
    assert!(std::ptr::eq(old.as_ref(), key.key_material()));
    assert!(key.key_material().image_key().is_none());
    drop(key);
    drop(held);
    let revision = state
        .update_keys(Some(old.revision()), image_change())
        .await
        .unwrap();
    assert_eq!(revision, old.revision() + 1);
    assert_eq!(loads(&state), 1);
    assert_eq!(updates(&state), 2);
}

#[tokio::test]
async fn cancelled_caller_still_drains_leases_and_publishes_the_committed_generation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let key = state.key_snapshot().await.unwrap();
    let revision = key.key_material().revision();
    let (entered, release) = probe(&state, false);
    let writer = state.clone();
    let caller =
        tokio::spawn(async move { writer.update_keys(Some(revision), image_change()).await });
    tokio::time::timeout(DEADLINE, state.update_waiting.notified())
        .await
        .unwrap();
    assert_eq!(updates(&state), 0);
    drop(query);
    assert!(
        tokio::time::timeout(Duration::from_millis(40), state.update_locked.notified())
            .await
            .is_err()
    );
    assert_eq!(updates(&state), 0);
    assert_eq!(key.key_material().revision(), revision);
    drop(key);
    tokio::time::timeout(DEADLINE, entered)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Store::for_runtime(&runtime)
            .unwrap()
            .load()
            .unwrap()
            .revision(),
        revision + 1
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut next_key = Box::pin(state.key_snapshot());
    let mut next_query = Box::pin(state.snapshot());
    std::future::poll_fn(|cx| {
        assert!(next_key.as_mut().poll(cx).is_pending());
        assert!(next_query.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    let key = tokio::time::timeout(DEADLINE, next_key)
        .await
        .unwrap()
        .unwrap();
    let query = tokio::time::timeout(DEADLINE, next_query)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(key.key_material().revision(), revision + 1);
    assert_eq!(
        key.key_material().image_key(),
        Some((*b"syntheticAESkey1", 0x88))
    );
    assert!(std::ptr::eq(key.key_material(), query.key_material()));
    assert_eq!(updates(&state), 1);
    assert_eq!(loads(&state), 1);
}

#[tokio::test]
async fn update_waits_for_cache_work_before_persisting() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime));
    assert!(state.snapshot().await.is_ok());
    let pending = state.cache_work.lock().await;
    let writer = state.clone();
    let caller = tokio::spawn(async move { writer.update_keys(None, image_change()).await });
    tokio::time::timeout(DEADLINE, state.update_locked.notified())
        .await
        .unwrap();
    assert_eq!(updates(&state), 0);
    let mut key = Box::pin(state.key_snapshot());
    std::future::poll_fn(|cx| {
        assert!(key.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(pending);
    let revision = tokio::time::timeout(DEADLINE, caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(key.await.unwrap().key_material().revision(), revision);
    assert_eq!(updates(&state), 1);
    assert_eq!(loads(&state), 1);
}

#[tokio::test]
async fn concurrent_cas_updates_publish_only_the_winner() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let revision = query.key_material().revision();
    drop(query);
    let (first, second) = tokio::join!(
        state.update_keys(Some(revision), image_change()),
        state.update_keys(
            Some(revision),
            vec![KeyChange::Image([0x55; 16], 0x99, Verification::Verified)]
        )
    );
    let (published, error, image) = match (first, second) {
        (Ok(revision), Err(error)) => (revision, error, (*b"syntheticAESkey1", 0x88)),
        (Err(error), Ok(revision)) => (revision, error, ([0x55; 16], 0x99)),
        results => panic!("expected one CAS winner: {results:?}"),
    };
    assert_eq!(published, revision + 1);
    assert_eq!(error.downcast_ref::<Error>(), Some(&Error::Conflict));
    let (key, query) = tokio::join!(state.key_snapshot(), state.snapshot());
    let key = key.unwrap();
    let query = query.unwrap();
    assert_eq!(key.key_material().revision(), published);
    assert_eq!(key.key_material().image_key(), Some(image));
    assert!(std::ptr::eq(key.key_material(), query.key_material()));
    assert_eq!(
        Store::for_runtime(&runtime)
            .unwrap()
            .load()
            .unwrap()
            .image_key(),
        Some(image)
    );
    assert_eq!(updates(&state), 2);
    assert_eq!(loads(&state), 1);
}

#[tokio::test]
async fn shutdown_waits_for_started_commit_and_restart_reads_it() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    assert!(state.snapshot().await.is_ok());
    let (entered, release) = probe(&state, false);
    let writer = state.clone();
    let caller = tokio::spawn(async move { writer.update_keys(None, image_change()).await });
    tokio::time::timeout(DEADLINE, entered)
        .await
        .unwrap()
        .unwrap();
    let mut shutdown = Box::pin(state.shutdown());
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    let revision = tokio::time::timeout(DEADLINE, caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(DEADLINE, shutdown).await.unwrap();
    assert!(state.snapshot.read().await.get().is_none());
    assert!(state.keys.read().await.get().is_none());
    assert!(state.key_snapshot().await.is_err());
    assert!(state.snapshot().await.is_err());
    assert!(state.update_keys(None, image_change()).await.is_err());
    assert_eq!(updates(&state), 1);
    let restarted = QueryState::new(runtime);
    let key = restarted.key_snapshot().await.unwrap();
    assert_eq!(key.key_material().revision(), revision);
    assert_eq!(
        key.key_material().image_key(),
        Some((*b"syntheticAESkey1", 0x88))
    );
}

#[tokio::test]
async fn shutdown_rejects_an_update_waiting_for_a_lease() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let path = runtime.config.key_store.as_ref().unwrap();
    let disk = fs::read(path).unwrap();
    let writer = state.clone();
    let caller = tokio::spawn(async move { writer.update_keys(None, image_change()).await });
    tokio::time::timeout(DEADLINE, state.update_waiting.notified())
        .await
        .unwrap();
    let mut shutdown = Box::pin(state.shutdown());
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(query);
    let failure = tokio::time::timeout(DEADLINE, caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(failure.to_string(), "Query service is stopping");
    tokio::time::timeout(DEADLINE, shutdown).await.unwrap();
    assert_eq!(updates(&state), 0);
    assert_eq!(fs::read(path).unwrap(), disk);
}

#[tokio::test]
async fn post_commit_panic_forces_explicit_reload_instead_of_serving_old_material() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let revision = query.key_material().revision();
    drop(query);
    let (entered, release) = probe(&state, true);
    let writer = state.clone();
    let caller =
        tokio::spawn(async move { writer.update_keys(Some(revision), image_change()).await });
    tokio::time::timeout(DEADLINE, entered)
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    let failure = tokio::time::timeout(DEADLINE, caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(failure
        .downcast_ref::<tokio::task::JoinError>()
        .unwrap()
        .is_panic());
    assert!(state.snapshot.read().await.get().is_none());
    assert!(state.keys.read().await.get().is_none());
    assert_eq!(loads(&state), 1);
    let key = state.key_snapshot().await.unwrap();
    assert_eq!(key.key_material().revision(), revision + 1);
    assert_eq!(
        key.key_material().image_key(),
        Some((*b"syntheticAESkey1", 0x88))
    );
    assert_eq!(loads(&state), 2);
    assert_eq!(updates(&state), 1);
    drop(key);
    assert!(state.snapshot().await.is_ok());
    assert_eq!(loads(&state), 2);
}

#[tokio::test]
async fn configuration_changed_after_persist_rejects_publication_without_rolling_back() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let original_config = fs::read(&runtime.config_path).unwrap();
    let state = Arc::new(QueryState::new(runtime.clone()));
    let query = state.snapshot().await.unwrap();
    let revision = query.key_material().revision();
    drop(query);
    let (entered, release) = probe(&state, false);
    let writer = state.clone();
    let caller =
        tokio::spawn(async move { writer.update_keys(Some(revision), image_change()).await });
    tokio::time::timeout(DEADLINE, entered)
        .await
        .unwrap()
        .unwrap();
    let path = runtime.config.key_store.as_ref().unwrap();
    let persisted = fs::read(path).unwrap();
    let store = Store::for_runtime(&runtime).unwrap();
    assert_eq!(store.load().unwrap().revision(), revision + 1);
    fs::write(
        &runtime.config_path,
        r#"{"db_dir":"other-account","keys_file":"other-account.json","key_store":"other-account.dpapi"}"#,
    )
    .unwrap();
    release.send(()).unwrap();
    let failure = tokio::time::timeout(DEADLINE, caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(failure
        .to_string()
        .contains("Account configuration changed"));
    assert!(state.snapshot.read().await.get().is_none());
    assert!(state.keys.read().await.get().is_none());
    assert_eq!(fs::read(path).unwrap(), persisted);
    assert_eq!(
        store.load().unwrap().image_key(),
        Some((*b"syntheticAESkey1", 0x88))
    );
    assert!(!root.path().join("other-account.dpapi").exists());
    assert!(state.key_snapshot().await.is_err());
    assert!(state.snapshot().await.is_err());
    assert_eq!(loads(&state), 1);
    assert_eq!(updates(&state), 1);
    fs::write(&runtime.config_path, original_config).unwrap();
    let restarted = QueryState::new(runtime);
    let key = restarted.key_snapshot().await.unwrap();
    assert_eq!(key.key_material().revision(), revision + 1);
    assert_eq!(
        key.key_material().image_key(),
        Some((*b"syntheticAESkey1", 0x88))
    );
}

#[tokio::test]
async fn account_configuration_change_rejects_update_before_store_access() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    seed(&runtime);
    let state = Arc::new(QueryState::new(runtime.clone()));
    assert!(state.snapshot().await.is_ok());
    let path = runtime.config.key_store.as_ref().unwrap();
    let disk = fs::read(path).unwrap();
    fs::write(
        &runtime.config_path,
        r#"{"db_dir":"other-account","keys_file":"keys.json","key_store":"keys.dpapi"}"#,
    )
    .unwrap();
    let failure = state.update_keys(None, image_change()).await.unwrap_err();
    assert!(failure
        .to_string()
        .contains("Account configuration changed"));
    assert_eq!(updates(&state), 0);
    assert_eq!(fs::read(path).unwrap(), disk);
}
