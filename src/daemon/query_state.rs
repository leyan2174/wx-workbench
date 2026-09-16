use anyhow::{ensure, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{Mutex, OnceCell, RwLock, RwLockReadGuard};

use super::{cache::DbCache, query, query::Names};
use crate::runtime::RuntimeContext;

pub enum KeyChange {
    Account(Vec<u8>, crate::key_store::Verification),
    Databases(
        std::collections::HashMap<String, String>,
        crate::key_store::Verification,
    ),
    Image([u8; 16], u8, crate::key_store::Verification),
}

impl KeyChange {
    fn borrowed(&self) -> crate::key_store::Update<'_> {
        match self {
            Self::Account(key, verification) => {
                crate::key_store::Update::Account(key, *verification)
            }
            Self::Databases(keys, verification) => {
                crate::key_store::Update::Databases(keys, *verification)
            }
            Self::Image(aes, xor, verification) => {
                crate::key_store::Update::Image(aes, *xor, *verification)
            }
        }
    }
}

impl Drop for KeyChange {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        match self {
            Self::Account(key, _) => key.zeroize(),
            Self::Databases(keys, _) => {
                for key in keys.values_mut() {
                    key.zeroize();
                }
            }
            Self::Image(aes, xor, _) => {
                aes.zeroize();
                xor.zeroize();
            }
        }
    }
}

struct QuerySnapshot {
    key_material: Arc<crate::key_store::Snapshot>,
    db: DbCache,
    names: RwLock<Arc<Names>>,
}

// 持久化一旦开始，异常退出必须丢弃旧代际；普通失败仅在磁盘确认未变时解除保护。
struct KeyPublication<'a> {
    query: &'a mut OnceCell<QuerySnapshot>,
    keys: &'a mut OnceCell<Arc<crate::key_store::Snapshot>>,
    reload_on_drop: bool,
}

impl Drop for KeyPublication<'_> {
    fn drop(&mut self) {
        if self.reload_on_drop {
            drop(self.query.take());
            drop(self.keys.take());
        }
    }
}

#[cfg(test)]
struct KeyUpdateProbe {
    committed: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    panic_after_commit: bool,
}

/// 查询期间持有当前代际的读锁，直到查询返回及其同步等待的缓存写入完成。
/// 调用取消后仍在运行的缓存提交由 cache_work 单独跟踪，失效和关闭也会等待它。
pub struct QueryLease<'a> {
    snapshot: RwLockReadGuard<'a, QuerySnapshot>,
}

impl QueryLease<'_> {
    pub fn key_material(&self) -> &crate::key_store::Snapshot {
        &self.snapshot.key_material
    }

    pub fn db(&self) -> &DbCache {
        &self.snapshot.db
    }

    pub fn names(&self) -> &RwLock<Arc<Names>> {
        &self.snapshot.names
    }
}

/// 仅持有密钥代际，不初始化数据库。释放后才能请求失效或获取查询租约。
pub struct KeyLease<'a> {
    snapshot: RwLockReadGuard<'a, Arc<crate::key_store::Snapshot>>,
}

impl KeyLease<'_> {
    pub fn key_material(&self) -> &crate::key_store::Snapshot {
        &self.snapshot
    }
}

pub struct QueryState {
    runtime: RuntimeContext,
    snapshot: RwLock<OnceCell<QuerySnapshot>>,
    keys: RwLock<OnceCell<Arc<crate::key_store::Snapshot>>>,
    cache_work: Arc<Mutex<()>>,
    stopped: AtomicBool,
    #[cfg(test)]
    lease_acquired: tokio::sync::Notify,
    #[cfg(test)]
    key_loads: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    key_updates: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    update_probe: std::sync::Mutex<Option<KeyUpdateProbe>>,
    #[cfg(test)]
    update_waiting: tokio::sync::Notify,
    #[cfg(test)]
    update_locked: tokio::sync::Notify,
}

impl QueryState {
    pub(super) fn runtime(&self) -> &RuntimeContext {
        &self.runtime
    }

    pub fn new(runtime: RuntimeContext) -> Self {
        Self {
            runtime,
            snapshot: RwLock::new(OnceCell::new()),
            keys: RwLock::new(OnceCell::new()),
            cache_work: Arc::new(Mutex::new(())),
            stopped: AtomicBool::new(false),
            #[cfg(test)]
            lease_acquired: tokio::sync::Notify::new(),
            #[cfg(test)]
            key_loads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            key_updates: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            update_probe: std::sync::Mutex::new(None),
            #[cfg(test)]
            update_waiting: tokio::sync::Notify::new(),
            #[cfg(test)]
            update_locked: tokio::sync::Notify::new(),
        }
    }

    pub async fn snapshot(&self) -> Result<QueryLease<'_>> {
        let cell = self.snapshot.read().await;
        self.validate().await?;
        cell.get_or_try_init(|| self.initialize()).await?;
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Query service is stopping"
        );
        #[cfg(test)]
        self.lease_acquired.notify_waiters();
        Ok(QueryLease {
            snapshot: RwLockReadGuard::map(cell, |cell| {
                cell.get()
                    .expect("query generation initialized under read lock")
            }),
        })
    }

    /// 每次校验固定账号配置；磁盘密钥替换仅在 invalidate_keys 或重启后加载。
    pub async fn key_snapshot(&self) -> Result<KeyLease<'_>> {
        let cell = self.keys.read().await;
        self.validate().await?;
        cell.get_or_try_init(|| self.load_keys()).await?;
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Query service is stopping"
        );
        Ok(KeyLease {
            snapshot: RwLockReadGuard::map(cell, |cell| {
                cell.get()
                    .expect("key generation initialized under read lock")
            }),
        })
    }

    async fn validate(&self) -> Result<()> {
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Query service is stopping"
        );
        let expected = self.runtime.clone();
        tokio::task::spawn_blocking(move || validated_runtime(&expected)).await??;
        Ok(())
    }

    async fn load_keys(&self) -> Result<Arc<crate::key_store::Snapshot>> {
        let runtime = self.runtime.clone();
        #[cfg(test)]
        let loads = self.key_loads.clone();
        tokio::task::spawn_blocking(move || {
            let store = crate::key_store::Store::for_runtime(&runtime)?;
            #[cfg(test)]
            loads.fetch_add(1, Ordering::Relaxed);
            Ok::<_, anyhow::Error>(Arc::new(store.load()?))
        })
        .await?
    }

    /// 原子持久化并发布密钥代际；调用前必须释放自身的查询及密钥租约。
    /// 取消等待不会取消已接收的事务。返回值是已发布快照的 revision。
    pub async fn update_keys(
        self: &Arc<Self>,
        expected_revision: Option<u64>,
        changes: Vec<KeyChange>,
    ) -> Result<u64> {
        let state = Arc::clone(self);
        tokio::spawn(async move { state.commit_keys(expected_revision, changes).await }).await?
    }

    async fn commit_keys(
        &self,
        expected_revision: Option<u64>,
        changes: Vec<KeyChange>,
    ) -> Result<u64> {
        #[cfg(test)]
        self.update_waiting.notify_one();
        let mut query = self.snapshot.write().await;
        let mut keys = self.keys.write().await;
        #[cfg(test)]
        self.update_locked.notify_one();
        let _pending = self.cache_work.lock().await;
        self.validate().await?;
        ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Query service is stopping"
        );
        let runtime = self.runtime.clone();
        #[cfg(test)]
        let updates = self.key_updates.clone();
        #[cfg(test)]
        let probe = self.update_probe.lock().unwrap().take();
        let mut publication = KeyPublication {
            query: &mut query,
            keys: &mut keys,
            reload_on_drop: true,
        };
        let (result, unchanged_on_error) = tokio::task::spawn_blocking(move || {
            use crate::infrastructure::configuration::Snapshot as FileSnapshot;
            let attempt = || -> Result<_> {
                let store = crate::key_store::Store::for_runtime(&runtime)?;
                let before = FileSnapshot::capture(store.path())?;
                let borrowed: Vec<_> = changes.iter().map(KeyChange::borrowed).collect();
                #[cfg(test)]
                updates.fetch_add(1, Ordering::Relaxed);
                let result = store.update(expected_revision, &borrowed);
                let unchanged_on_error = result.is_err()
                    && FileSnapshot::capture(store.path())
                        .is_ok_and(|after| before.bytes() == after.bytes());
                #[cfg(test)]
                if result.is_ok() {
                    if let Some(probe) = probe {
                        let _ = probe.committed.send(());
                        probe
                            .release
                            .recv_timeout(std::time::Duration::from_secs(10))
                            .unwrap();
                        assert!(!probe.panic_after_commit, "synthetic publication failure");
                    }
                }
                Ok((result.map_err(anyhow::Error::from), unchanged_on_error))
            };
            // Store 构造或文件快照失败发生在 update 调用之前，没有本事务的写入。
            attempt().unwrap_or_else(|error| (Err(error), true))
        })
        .await?;
        match result {
            Ok(snapshot) => {
                // 保存期间配置可能变化；发布前再次确认固定账号，失败由 guard 清空旧代际。
                // 已接收事务仍需在 shutdown 等待的屏障内完成，不在此重复检查 stopped。
                let expected = self.runtime.clone();
                tokio::task::spawn_blocking(move || validated_runtime(&expected)).await??;
                let revision = snapshot.revision();
                let snapshot = Arc::new(snapshot);
                drop(publication.query.take());
                *publication.keys = OnceCell::from(snapshot);
                publication.reload_on_drop = false;
                Ok(revision)
            }
            Err(error) => {
                publication.reload_on_drop = !unchanged_on_error;
                Err(error)
            }
        }
    }

    /// 等待已有查询及取消后仍在提交的缓存任务结束，再丢弃旧代际。
    /// 仅刷新查询缓存，保留密钥；调用时不能持有查询租约。
    #[cfg(test)]
    pub async fn invalidate(&self) {
        let mut cell = self.snapshot.write().await;
        let _pending = self.cache_work.lock().await;
        let previous = cell.take();
        drop(previous);
    }

    /// 密钥更新后调用，下一次访问惰性重载。锁顺序固定为 query -> key -> cache_work。
    /// 调用者必须先释放查询及密钥租约，不能持有租约等待本方法。
    pub async fn invalidate_keys(&self) {
        let mut query = self.snapshot.write().await;
        let mut keys = self.keys.write().await;
        let _pending = self.cache_work.lock().await;
        drop(query.take());
        drop(keys.take());
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.invalidate_keys().await;
    }

    async fn initialize(&self) -> Result<QuerySnapshot> {
        let lease = self.key_snapshot().await?;
        let key_material = Arc::clone(&lease.snapshot);
        drop(lease);
        let all_keys = key_material.database_keys();
        ensure!(!all_keys.is_empty(), "Query keys are unavailable");
        let runtime = &self.runtime;

        use crate::adapters::wechat::messages::inventory::configured_sources;
        use crate::business::messages::SourceKind;
        let msg_db_keys =
            configured_sources(all_keys.keys().map(String::as_str), SourceKind::Ordinary);
        let biz_msg_db_keys = configured_sources(
            all_keys.keys().map(String::as_str),
            SourceKind::OfficialPush,
        );
        let db = DbCache::with_dirs_coordinated(
            runtime.config.db_dir.clone(),
            runtime.cache_dir(),
            runtime.mtime_file(),
            all_keys,
            self.cache_work.clone(),
        )
        .await?;
        let mut names =
            query::load_names_with_retry(&db, 5, std::time::Duration::from_millis(300)).await?;
        names.msg_db_keys = msg_db_keys;
        names.biz_msg_db_keys = biz_msg_db_keys;
        let _ = db
            .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
            .await;
        let _ = db.get("sns/sns.db").await;
        Ok(QuerySnapshot {
            key_material,
            db,
            names: RwLock::new(Arc::new(names)),
        })
    }
}

#[cfg(test)]
#[path = "query_extract_tests.rs"]
mod extract_tests;

fn validated_runtime(expected: &RuntimeContext) -> Result<RuntimeContext> {
    let config = crate::config::load_config_at(&expected.config_path)?;
    let current =
        RuntimeContext::from_config(expected.config_path.clone(), config, expected.root.clone())?;
    ensure!(
        current.same_account(expected)?,
        "Account configuration changed; restart the daemon"
    );
    Ok(current)
}

#[cfg(test)]
#[path = "query_state_update_tests.rs"]
mod update_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::query::encrypted_cache;
    use crate::daemon::server::{
        dispatch_state, read_initial_request_frame, read_request_frame, MAX_REQUEST_FRAME_BYTES,
    };
    use crate::ipc::Request;
    use serde_json::json;
    use std::{fs, future::Future, path::Path, task::Poll};
    use tokio::io::{AsyncWriteExt, BufReader};

    pub(super) fn runtime(root: &Path) -> RuntimeContext {
        let path = root.join("config.json");
        fs::write(
            &path,
            json!({"db_dir":"db_storage", "keys_file":"keys.json", "key_store":"keys.dpapi"})
                .to_string(),
        )
        .unwrap();
        RuntimeContext::from_config(
            path.clone(),
            crate::config::load_config_at(&path).unwrap(),
            root.join("home"),
        )
        .unwrap()
    }

    pub(super) fn seed(runtime: &RuntimeContext) {
        let source = runtime.config.db_dir.join("contact/contact.db");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(runtime.cache_dir()).unwrap();
        let cached = runtime.cache_dir().join("contact.db");
        let connection = encrypted_cache::sqlite(&cached);
        connection.execute_batch(
            "CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER);
             INSERT INTO contact VALUES('wxid_test','Test','',0);",
        ).unwrap();
        drop(connection);
        let mt = encrypted_cache::seed(&cached, &source);
        fs::write(
            runtime.mtime_file(),
            json!({"contact/contact.db":{"db_mt":mt,"wal_mt":0,"path":cached}}).to_string(),
        )
        .unwrap();
        crate::key_store::seed_databases(
            runtime,
            json!({"contact/contact.db":"11".repeat(32),
                "message/message_0.db":"22".repeat(32),
                "message/biz_message_0.db":"33".repeat(32)}),
        );
    }

    fn loads(state: &QueryState) -> usize {
        state.key_loads.load(Ordering::Relaxed)
    }

    #[tokio::test]
    async fn key_loads_are_shared_across_first_concurrent_and_repeated_hosts() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        seed(&runtime);
        let state = QueryState::new(runtime);
        let (key, query, other) =
            tokio::join!(state.key_snapshot(), state.snapshot(), state.key_snapshot());
        let key = key.unwrap();
        let query = query.unwrap();
        let other = other.unwrap();
        assert!(std::ptr::eq(key.key_material(), query.key_material()));
        assert!(std::ptr::eq(key.key_material(), other.key_material()));
        assert_eq!(loads(&state), 1);
        drop((key, query, other));
        for _ in 0..4 {
            let (key, query) = tokio::join!(state.key_snapshot(), state.snapshot());
            assert!(std::ptr::eq(
                key.unwrap().key_material(),
                query.unwrap().key_material()
            ));
        }
        assert_eq!(loads(&state), 1);
        state.invalidate().await;
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 1);
        state.invalidate_keys().await;
        assert_eq!(loads(&state), 1);
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 2);
    }

    #[tokio::test]
    async fn image_only_keys_do_not_require_query_initialization() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::create_dir_all(&runtime.config.db_dir).unwrap();
        crate::key_store::Store::for_runtime(&runtime)
            .unwrap()
            .update(
                None,
                &[crate::key_store::Update::Image(
                    b"syntheticAESkey1",
                    0x88,
                    crate::key_store::Verification::Verified,
                )],
            )
            .unwrap();
        let state = QueryState::new(runtime.clone());
        let lease = state.key_snapshot().await.unwrap();
        assert_eq!(
            lease.key_material().image_key(),
            Some((*b"syntheticAESkey1", 0x88))
        );
        assert!(lease.key_material().database_keys().is_empty());
        assert!(state.snapshot.read().await.get().is_none());
        assert!(!runtime.cache_dir().exists());
        drop(lease);
        for _ in 0..2 {
            let error = state.snapshot().await.err().unwrap();
            assert_eq!(error.to_string(), "Query keys are unavailable");
            assert!(state.snapshot.read().await.get().is_none());
            assert!(state.key_snapshot().await.is_ok());
            assert_eq!(loads(&state), 1);
        }
        assert!(!runtime.cache_dir().exists());
    }

    #[tokio::test]
    async fn actual_key_load_failures_retry_and_corruption_is_detected_only_after_invalidation() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::create_dir_all(&runtime.config.db_dir).unwrap();
        let state = QueryState::new(runtime.clone());
        let path = runtime.config.key_store.as_ref().unwrap();
        for (index, content) in [None, Some("secret-invalid-json"), Some("{}")]
            .into_iter()
            .enumerate()
        {
            if let Some(content) = content {
                fs::write(path, content).unwrap();
            }
            assert!(state.key_snapshot().await.is_err());
            assert!(state.keys.read().await.get().is_none());
            assert_eq!(loads(&state), index + 1);
        }
        fs::remove_file(path).unwrap();
        seed(&runtime);
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 4);
        fs::write(path, "secret-invalid-json").unwrap();
        assert!(state.key_snapshot().await.is_ok());
        assert!(state.snapshot().await.is_ok());
        state.invalidate().await;
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 4);
        state.invalidate_keys().await;
        assert!(state.key_snapshot().await.is_err());
        assert_eq!(loads(&state), 5);
        assert!(state.snapshot().await.is_err());
        assert_eq!(loads(&state), 6);
        assert!(state.keys.read().await.get().is_none());
        assert!(state.snapshot.read().await.get().is_none());
        fs::remove_file(path).unwrap();
        // 数据库夹具仍然有效，恢复时只重建损坏的密钥库。
        crate::key_store::seed_databases(
            &runtime,
            json!({"contact/contact.db":"11".repeat(32),
                "message/message_0.db":"22".repeat(32),
                "message/biz_message_0.db":"33".repeat(32)}),
        );
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 7);
    }

    #[tokio::test]
    async fn key_invalidation_and_shutdown_drain_both_kinds_of_lease() {
        for stop in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let runtime = runtime(root.path());
            seed(&runtime);
            let state = QueryState::new(runtime);
            let query = state.snapshot().await.unwrap();
            let key = state.key_snapshot().await.unwrap();
            let mut drain = Box::pin(async {
                if stop {
                    state.shutdown().await;
                } else {
                    state.invalidate_keys().await;
                }
            });
            std::future::poll_fn(|cx| {
                assert!(drain.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(query);
            std::future::poll_fn(|cx| {
                assert!(drain.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            let mut next = Box::pin(state.key_snapshot());
            std::future::poll_fn(|cx| {
                assert!(next.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(dispatch_state(Request::Ping, &state).await.ok);
            drop(key);
            tokio::time::timeout(std::time::Duration::from_secs(5), drain)
                .await
                .unwrap();
            assert!(state.keys.read().await.get().is_none());
            assert!(state.snapshot.read().await.get().is_none());
            if stop {
                assert!(next.await.is_err());
                assert!(state.snapshot().await.is_err());
                assert_eq!(loads(&state), 1);
            } else {
                assert!(next.await.is_ok());
                assert_eq!(loads(&state), 2);
            }
        }
    }

    #[tokio::test]
    async fn ping_is_lazy_and_failures_are_safe_and_retryable() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let state = QueryState::new(runtime.clone());
        let ping = dispatch_state(Request::Ping, &state).await;
        assert!(ping.ok);
        assert_eq!(ping.data["pong"], true);
        assert!(state.snapshot.read().await.get().is_none());
        assert!(!runtime.cache_dir().exists());
        assert_eq!(loads(&state), 0);

        for content in [None, Some("secret-invalid-json"), Some("{}")] {
            if let Some(content) = content {
                fs::write(runtime.config.key_store.as_ref().unwrap(), content).unwrap();
            }
            let response = dispatch_state(Request::ContactTags, &state).await;
            assert!(!response.ok);
            assert_eq!(
                response.error.as_deref(),
                Some("Key store file or path validation failed")
            );
            assert!(state.snapshot.read().await.get().is_none());
        }

        fs::remove_file(runtime.config.key_store.as_ref().unwrap()).unwrap();
        seed(&runtime);
        let (first, second) = tokio::join!(state.snapshot(), state.snapshot());
        let first = first.unwrap();
        let second = second.unwrap();
        assert!(std::ptr::eq(first.db(), second.db()));
        assert!(std::ptr::eq(first.key_material(), second.key_material()));
        assert_eq!(loads(&state), 1);
        let names = first.names().read().await;
        assert_eq!(names.map["wxid_test"], "Test");
        assert_eq!(names.msg_db_keys, ["message/message_0.db"]);
        assert_eq!(names.biz_msg_db_keys, ["message/biz_message_0.db"]);
        drop(names);
        drop(first);
        drop(second);
        state.invalidate().await;
        assert!(state.snapshot.read().await.get().is_none());
        let fresh = state.snapshot().await.unwrap();
        assert_eq!(fresh.names().read().await.map["wxid_test"], "Test");
        assert_eq!(loads(&state), 1);
    }

    #[tokio::test]
    async fn invalidation_drains_active_leases_and_queues_new_queries_but_not_ping() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        seed(&runtime);
        let state = QueryState::new(runtime.clone());
        let active = state.snapshot().await.unwrap();
        crate::key_store::seed_databases(
            &runtime,
            json!({
                "contact/contact.db":"11".repeat(32),
                "message/message_1.db":"22".repeat(32)
            }),
        );

        let mut invalidate = Box::pin(state.invalidate_keys());
        std::future::poll_fn(|cx| {
            assert!(invalidate.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let mut next = Box::pin(state.snapshot());
        std::future::poll_fn(|cx| {
            assert!(next.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(dispatch_state(Request::Ping, &state).await.ok);
        assert_eq!(
            active.names().read().await.msg_db_keys,
            ["message/message_0.db"]
        );

        drop(active);
        invalidate.await;
        let fresh = next.await.unwrap();
        assert_eq!(
            fresh.names().read().await.msg_db_keys,
            ["message/message_1.db"]
        );
        assert_eq!(loads(&state), 2);
    }

    #[tokio::test]
    async fn dispatch_retains_its_generation_lease_until_query_returns() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        seed(&runtime);
        let state = QueryState::new(runtime);
        let active = state.snapshot().await.unwrap();
        let names_writer = active.names().write().await;
        let mut request = Box::pin(dispatch_state(
            Request::ResolveChat {
                chat: "wxid_test".into(),
            },
            &state,
        ));
        let acquired = state.lease_acquired.notified();
        tokio::pin!(acquired);
        tokio::select! {
            biased;
            _ = &mut acquired => {},
            _ = &mut request => panic!("query completed while names were write-locked"),
        }
        drop(names_writer);
        drop(active);
        let mut invalidate = Box::pin(state.invalidate());
        std::future::poll_fn(|cx| {
            assert!(invalidate.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(request.await.ok);
        invalidate.await;
        assert!(state.snapshot.read().await.get().is_none());
    }

    #[tokio::test]
    async fn key_revision_refresh_requires_explicit_invalidation_and_drains_leases() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        seed(&runtime);
        let state = QueryState::new(runtime.clone());
        let active = state.snapshot().await.unwrap();
        let store = crate::key_store::Store::for_runtime(&runtime).unwrap();
        let revision = store
            .update(
                None,
                &[crate::key_store::Update::Image(
                    b"syntheticAESkey1",
                    0x88,
                    crate::key_store::Verification::Verified,
                )],
            )
            .unwrap()
            .revision();
        assert_ne!(active.key_material().revision(), revision);
        let unchanged = state.snapshot().await.unwrap();
        assert!(std::ptr::eq(
            active.key_material(),
            unchanged.key_material()
        ));
        assert_eq!(loads(&state), 1);
        drop(unchanged);
        let mut invalidate = Box::pin(state.invalidate_keys());
        std::future::poll_fn(|cx| {
            assert!(invalidate.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let mut pending = Box::pin(state.snapshot());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(40), &mut pending)
                .await
                .is_err()
        );
        drop(active);
        invalidate.await;
        let fresh = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fresh.key_material().revision(), revision);
        assert_eq!(
            fresh.key_material().image_key(),
            Some((*b"syntheticAESkey1", 0x88))
        );
        assert_eq!(loads(&state), 2);
    }

    #[tokio::test]
    async fn cancelled_cache_commit_is_drained_before_invalidation_or_shutdown() {
        for mode in 0..3 {
            let stop = mode == 2;
            let root = tempfile::tempdir().unwrap();
            let runtime = runtime(root.path());
            seed(&runtime);
            let state = Arc::new(QueryState::new(runtime));
            let lease = state.snapshot().await.unwrap();
            assert!(lease.db().invalidate("contact/contact.db").await);
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            lease.db().set_before_commit(move || {
                entered_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            });
            drop(lease);
            let pending_state = state.clone();
            let pending = tokio::spawn(async move {
                let lease = pending_state.snapshot().await.unwrap();
                lease.db().get("contact/contact.db").await
            });
            tokio::time::timeout(std::time::Duration::from_secs(10), entered_rx)
                .await
                .unwrap()
                .unwrap();
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            let mut drain = Box::pin(async {
                if stop {
                    state.shutdown().await;
                } else if mode == 1 {
                    state.invalidate_keys().await;
                } else {
                    state.invalidate().await;
                }
            });
            std::future::poll_fn(|cx| {
                assert!(drain.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(dispatch_state(Request::Ping, &state).await.ok);
            let mut fresh = Box::pin(state.snapshot());
            std::future::poll_fn(|cx| {
                assert!(fresh.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            release_tx.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), drain)
                .await
                .unwrap();
            assert!(state.snapshot.read().await.get().is_none());
            if stop {
                assert!(fresh.await.is_err());
                assert_eq!(loads(&state), 1);
            } else {
                let fresh = fresh.await.unwrap();
                assert_eq!(loads(&state), if mode == 1 { 2 } else { 1 });
                assert_eq!(
                    fresh
                        .db()
                        .get_with_mode("contact/contact.db")
                        .await
                        .unwrap()
                        .unwrap()
                        .mode,
                    super::super::cache::CacheMode::CacheHit
                );
            }
        }
    }

    #[tokio::test]
    async fn initial_frame_timeout_does_not_wait_for_peer_to_close() {
        let (_writer, reader) = tokio::io::duplex(64);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(7),
            read_initial_request_frame(&mut BufReader::new(reader)),
        )
        .await
        .expect("initial frame deadline was not enforced")
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn changed_account_paths_are_rejected_before_cache_initialization() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let state = QueryState::new(runtime.clone());
        for config in [
            json!({"db_dir":"other-account", "keys_file":"keys.json"}),
            json!({"db_dir":"db_storage", "keys_file":"other-keys.json"}),
        ] {
            fs::write(&runtime.config_path, config.to_string()).unwrap();
            assert!(validated_runtime(&runtime).is_err());
            assert!(state.snapshot().await.is_err());
            assert!(state.key_snapshot().await.is_err());
            assert_eq!(loads(&state), 0);
            assert!(!runtime.cache_dir().exists());
        }
        fs::write(
            &runtime.config_path,
            json!({"db_dir":"db_storage", "keys_file":"keys.json", "key_store":"keys.dpapi"})
                .to_string(),
        )
        .unwrap();
        assert!(validated_runtime(&runtime).is_ok());
        seed(&runtime);
        assert!(state.snapshot().await.is_ok());
        assert_eq!(loads(&state), 1);
        let original = fs::read(&runtime.config_path).unwrap();
        for field in [
            "db_dir",
            "keys_file",
            "key_store",
            "decrypted_dir",
            "wechat_process",
        ] {
            let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
            changed[field] = json!("other-account");
            fs::write(&runtime.config_path, changed.to_string()).unwrap();
            assert!(
                validated_runtime(&runtime).is_err(),
                "accepted changed {field}"
            );
            assert!(
                state.snapshot().await.is_err(),
                "query accepted changed {field}"
            );
            assert!(
                state.key_snapshot().await.is_err(),
                "keys accepted changed {field}"
            );
            assert_eq!(loads(&state), 1);
        }
        fs::write(&runtime.config_path, "invalid-config").unwrap();
        assert!(state.snapshot().await.is_err());
        assert!(state.key_snapshot().await.is_err());
        assert_eq!(loads(&state), 1);
        fs::write(&runtime.config_path, original).unwrap();
        assert!(state.snapshot().await.is_ok());
        assert!(state.key_snapshot().await.is_ok());
        assert_eq!(loads(&state), 1);
    }

    #[tokio::test]
    async fn frame_reader_preserves_lines_eof_utf8_and_boundaries() {
        for bytes in [b"hello\n".as_slice(), b"hello\r\n"] {
            assert_eq!(
                read_request_frame(&mut BufReader::new(bytes))
                    .await
                    .unwrap(),
                Some("hello".to_owned())
            );
        }
        assert!(read_request_frame(&mut BufReader::new(b"hello".as_slice()))
            .await
            .is_err());
        assert!(read_request_frame(&mut BufReader::new(b"".as_slice()))
            .await
            .unwrap()
            .is_none());
        assert!(
            read_request_frame(&mut BufReader::new(b"\xff\n".as_slice()))
                .await
                .is_err()
        );
        let mut two_lines = BufReader::new(b"first\nsecond\n".as_slice());
        assert_eq!(
            read_request_frame(&mut two_lines).await.unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            read_request_frame(&mut two_lines).await.unwrap().as_deref(),
            Some("second")
        );
        let mut boundary = vec![b'x'; MAX_REQUEST_FRAME_BYTES];
        assert!(read_request_frame(&mut BufReader::new(boundary.as_slice()))
            .await
            .is_err());
        boundary[MAX_REQUEST_FRAME_BYTES - 1] = b'\n';
        assert!(read_request_frame(&mut BufReader::new(boundary.as_slice()))
            .await
            .is_ok());
        boundary.insert(0, b'x');
        assert!(read_request_frame(&mut BufReader::new(boundary.as_slice()))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn oversized_unterminated_frame_is_rejected_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(MAX_REQUEST_FRAME_BYTES + 1);
        writer
            .write_all(&vec![b'x'; MAX_REQUEST_FRAME_BYTES + 1])
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_request_frame(&mut BufReader::new(reader)),
        )
        .await
        .expect("oversized frame waited for EOF");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        drop(writer);
    }
}
