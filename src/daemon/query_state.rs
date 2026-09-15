use anyhow::{ensure, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{Mutex, OnceCell, RwLock, RwLockReadGuard};

use super::{cache::DbCache, query, query::Names};
use crate::runtime::RuntimeContext;

struct QuerySnapshot {
    key_revision: u64,
    db: DbCache,
    names: RwLock<Arc<Names>>,
}

/// 查询期间持有当前代际的读锁，直到查询返回及其同步等待的缓存写入完成。
/// 调用取消后仍在运行的缓存提交由 cache_work 单独跟踪，失效和关闭也会等待它。
pub struct QueryLease<'a> {
    snapshot: RwLockReadGuard<'a, QuerySnapshot>,
}

impl QueryLease<'_> {
    pub fn db(&self) -> &DbCache {
        &self.snapshot.db
    }

    pub fn names(&self) -> &RwLock<Arc<Names>> {
        &self.snapshot.names
    }
}

pub struct QueryState {
    runtime: RuntimeContext,
    snapshot: RwLock<OnceCell<QuerySnapshot>>,
    cache_work: Arc<Mutex<()>>,
    stopped: AtomicBool,
    #[cfg(test)]
    lease_acquired: tokio::sync::Notify,
}

impl QueryState {
    pub fn new(runtime: RuntimeContext) -> Self {
        Self {
            runtime,
            snapshot: RwLock::new(OnceCell::new()),
            cache_work: Arc::new(Mutex::new(())),
            stopped: AtomicBool::new(false),
            #[cfg(test)]
            lease_acquired: tokio::sync::Notify::new(),
        }
    }

    pub async fn snapshot(&self) -> Result<QueryLease<'_>> {
        loop {
            let expected = self.runtime.clone();
            let revision = tokio::task::spawn_blocking(move || -> Result<u64> {
                let runtime = validated_runtime(&expected)?;
                Ok(crate::key_store::Store::for_runtime(&runtime)?
                    .load()?
                    .revision())
            })
            .await??;
            let cell = self.snapshot.read().await;
            ensure!(
                !self.stopped.load(Ordering::Acquire),
                "Query service is stopping"
            );
            cell.get_or_try_init(|| self.initialize()).await?;
            if cell
                .get()
                .is_some_and(|snapshot| snapshot.key_revision != revision)
            {
                drop(cell);
                self.invalidate().await;
                continue;
            }
            ensure!(
                !self.stopped.load(Ordering::Acquire),
                "Query service is stopping"
            );
            #[cfg(test)]
            self.lease_acquired.notify_waiters();
            return Ok(QueryLease {
                snapshot: RwLockReadGuard::map(cell, |cell| {
                    cell.get()
                        .expect("query generation initialized under read lock")
                }),
            });
        }
    }

    /// 等待已有查询及取消后仍在提交的缓存任务结束，再丢弃旧代际。
    /// 密钥刷新成功后调用；等待时不能持有查询租约，否则会阻塞自身。
    pub async fn invalidate(&self) {
        let mut cell = self.snapshot.write().await;
        let _pending = self.cache_work.lock().await;
        let previous = cell.take();
        drop(previous);
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.invalidate().await;
    }

    async fn initialize(&self) -> Result<QuerySnapshot> {
        let expected = self.runtime.clone();
        let (runtime, all_keys, key_revision) = tokio::task::spawn_blocking(move || {
            let runtime = validated_runtime(&expected)?;
            let snapshot = crate::key_store::Store::for_runtime(&runtime)?.load()?;
            let keys = snapshot.database_keys();
            ensure!(!keys.is_empty(), "Query keys are unavailable");
            Ok::<_, anyhow::Error>((runtime, keys, snapshot.revision()))
        })
        .await??;

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
            key_revision,
            db,
            names: RwLock::new(Arc::new(names)),
        })
    }
}

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

    fn runtime(root: &Path) -> RuntimeContext {
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

    fn seed(runtime: &RuntimeContext) {
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

        let mut invalidate = Box::pin(state.invalidate());
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
    async fn key_revision_refresh_drops_its_read_lease_before_waiting() {
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
        assert_ne!(active.snapshot.key_revision, revision);
        let mut pending = Box::pin(state.snapshot());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(40), &mut pending)
                .await
                .is_err()
        );
        drop(active);
        let fresh = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fresh.snapshot.key_revision, revision);
    }

    #[tokio::test]
    async fn cancelled_cache_commit_is_drained_before_invalidation_or_shutdown() {
        for stop in [false, true] {
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
            } else {
                let fresh = fresh.await.unwrap();
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
