//! 认证服务内的大游标暂存。实例只属于一个账号，状态不落盘、不写日志。
use crate::service::protocol::{monitor as wire, ServiceError};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{watch, OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

const MAX_UPLOADS: usize = 2;
const TTL: Duration = Duration::from_secs(60);
type Result<T> = std::result::Result<T, ServiceError>;
fn error(code: &'static str) -> ServiceError {
    ServiceError::new(code, code)
}

struct Upload {
    expected_sessions: usize,
    expected_bytes: usize,
    bytes: usize,
    sequence: u32,
    sessions: HashMap<String, i64>,
    _permit: OwnedSemaphorePermit,
}
struct Slot {
    expires: Instant,
    upload: Option<Upload>,
    cancel: watch::Sender<bool>,
}

pub(crate) struct Service {
    slots: Mutex<HashMap<String, Slot>>,
    quota: Arc<Semaphore>,
}
impl Service {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Mutex::new(HashMap::new()),
            quota: Arc::new(Semaphore::new(MAX_UPLOADS)),
        })
    }

    /// 固定截止时间不随块刷新。即使客户端断线且不再发请求，也有定时清理。
    pub async fn reap(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                _ = interval.tick() => self.expire(),
                _ = shutdown.changed() => break,
            }
        }
        self.clear();
    }
    fn expire(&self) {
        self.slots.lock().unwrap().retain(|_, slot| {
            if slot.expires > Instant::now() {
                return true;
            }
            slot.cancel.send_replace(true);
            false
        });
    }
    pub fn clear(&self) {
        let mut slots = self.slots.lock().unwrap();
        for slot in slots.values() {
            slot.cancel.send_replace(true);
        }
        slots.clear();
    }

    #[cfg(test)]
    pub(crate) fn active_uploads(&self) -> usize {
        self.slots.lock().unwrap().len()
    }

    fn begin(&self, sessions: usize, bytes: usize) -> Result<Value> {
        if sessions > wire::MAX_SESSIONS
            || !(2..=wire::MAX_STATE_BYTES).contains(&bytes)
            || (sessions == 0 && bytes != 2)
        {
            return Err(error("monitor_state_limit"));
        }
        self.expire();
        let permit = self
            .quota
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("monitor_busy"))?;
        let id = random_id()?;
        let (cancel, _) = watch::channel(false);
        let mut slots = self.slots.lock().unwrap();
        if slots.contains_key(&id) {
            return Err(error("monitor_id_collision"));
        }
        slots.insert(
            id.clone(),
            Slot {
                expires: Instant::now() + TTL,
                cancel,
                upload: Some(Upload {
                    expected_sessions: sessions,
                    expected_bytes: bytes,
                    bytes: 2,
                    sequence: 0,
                    sessions: HashMap::new(),
                    _permit: permit,
                }),
            },
        );
        Ok(json!({"id":id}))
    }

    fn chunk(&self, id: &str, sequence: u32, entries: Vec<(String, i64)>) -> Result<Value> {
        self.expire();
        let mut slots = self.slots.lock().unwrap();
        let result = (|| {
            let slot = slots
                .get_mut(id)
                .ok_or_else(|| error("monitor_upload_missing"))?;
            let upload = slot
                .upload
                .as_mut()
                .ok_or_else(|| error("monitor_upload_consumed"))?;
            if entries.is_empty()
                || entries.len() > wire::MAX_CHUNK_ENTRIES
                || serde_json::to_vec(&entries)
                    .map_err(|_| error("monitor_chunk_invalid"))?
                    .len()
                    > wire::CHUNK_BYTES
                || sequence != upload.sequence
            {
                return Err(error("monitor_chunk_invalid"));
            }
            let latest = chrono::Utc::now().timestamp().saturating_add(86400);
            for (name, timestamp) in entries {
                if !wire::valid_entry(&name, timestamp, latest)
                    || upload.sessions.contains_key(&name)
                    || upload.sessions.len() >= upload.expected_sessions
                {
                    return Err(error("monitor_entry_invalid"));
                }
                let bytes = upload.bytes
                    + wire::entry_bytes(&name, timestamp)
                    + usize::from(!upload.sessions.is_empty());
                if bytes > upload.expected_bytes {
                    return Err(error("monitor_state_limit"));
                }
                // 保留解码后的实际长度，不让 JSON 转义产生的临时容量常驻。
                upload
                    .sessions
                    .insert(name.into_boxed_str().into_string(), timestamp);
                upload.bytes = bytes;
            }
            upload.sequence += 1;
            Ok(json!({"sequence":upload.sequence}))
        })();
        // 任一坏块使整个上传失效；不能保留半块后继续 Finish。
        if result.is_err() {
            if let Some(slot) = slots.remove(id) {
                slot.cancel.send_replace(true);
            }
        }
        result
    }

    fn abort(&self, id: &str) -> Value {
        if let Some(slot) = self.slots.lock().unwrap().remove(id) {
            slot.cancel.send_replace(true);
        }
        json!({"aborted":true})
    }

    /// 只有完整上传能拿到状态。租约持有配额直到查询结束或 future 被丢弃。
    fn take(self: &Arc<Self>, id: &str, chunks: u32) -> Result<Lease> {
        self.expire();
        let mut slots = self.slots.lock().unwrap();
        let slot = slots
            .get_mut(id)
            .ok_or_else(|| error("monitor_upload_missing"))?;
        let upload = slot
            .upload
            .take()
            .ok_or_else(|| error("monitor_upload_consumed"))?;
        if upload.sequence != chunks
            || upload.sessions.len() != upload.expected_sessions
            || upload.bytes != upload.expected_bytes
        {
            slots.remove(id);
            return Err(error("monitor_upload_incomplete"));
        }
        Ok(Lease {
            service: self.clone(),
            id: id.into(),
            expires: slot.expires,
            cancel: slot.cancel.subscribe(),
            upload,
        })
    }

    pub async fn handle<F, Fut>(self: &Arc<Self>, call: wire::Call, query: F) -> Result<Value>
    where
        F: FnOnce(crate::ipc::Request) -> Fut,
        Fut: Future<Output = crate::ipc::Response>,
    {
        match call {
            wire::Call::Begin { sessions, bytes } => self.begin(sessions, bytes),
            wire::Call::Chunk {
                id,
                sequence,
                entries,
            } => self.chunk(&id, sequence, entries),
            wire::Call::Abort { id } => Ok(self.abort(&id)),
            wire::Call::Finish {
                id,
                chunks,
                limit,
                with_meta,
                debug_source,
                max_response_bytes,
            } => {
                if !(1..=10_000).contains(&limit)
                    || !(1024..=wire::MAX_RESPONSE_BYTES).contains(&max_response_bytes)
                {
                    self.abort(&id);
                    return Err(error("monitor_query_limit"));
                }
                let mut lease = self.take(&id, chunks)?;
                let state = std::mem::take(&mut lease.upload.sessions);
                let request = crate::ipc::Request::NewMessages {
                    state: Some(state),
                    limit,
                    with_meta,
                    debug_source,
                };
                let response = tokio::select! {
                    biased;
                    _ = lease.cancel.changed() => return Err(error("monitor_cancelled")),
                    _ = tokio::time::sleep_until(lease.expires) => return Err(error("monitor_expired")),
                    response = query(request) => response,
                };
                if !response.ok {
                    return Err(error("monitor_query_failed"));
                }
                let response_bytes =
                    crate::service::transport::encode(&response, max_response_bytes)
                        .map_err(|_| error("monitor_response_limit"))?
                        .len();
                Ok(json!({"data":response.data,"response_bytes":response_bytes}))
            }
        }
    }
}
struct Lease {
    service: Arc<Service>,
    id: String,
    expires: Instant,
    cancel: watch::Receiver<bool>,
    upload: Upload,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.service.slots.lock().unwrap().remove(&self.id);
    }
}

fn random_id() -> Result<String> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut bytes = [0u8; 32];
    // 系统随机源，不使用进程号、时间或会话内容派生可猜测的上传 ID。
    unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut bytes,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    }
    .ok()
    .map_err(|_| error("monitor_random_failed"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{Request, Response};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn finish(id: &str, chunks: u32) -> wire::Call {
        wire::Call::Finish {
            id: id.into(),
            chunks,
            limit: 5,
            with_meta: true,
            debug_source: false,
            max_response_bytes: wire::MAX_RESPONSE_BYTES,
        }
    }
    fn begin(service: &Service, state: &HashMap<String, i64>) -> String {
        service
            .begin(state.len(), serde_json::to_vec(state).unwrap().len())
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn upload(service: &Service, state: HashMap<String, i64>) -> (String, u32) {
        let id = begin(service, &state);
        let expires = service.slots.lock().unwrap()[&id].expires;
        let mut iter = state.into_iter();
        let mut chunks = 0;
        loop {
            // 最长 512 字节 ID 时也小于每块预算。
            let entries: Vec<_> = iter.by_ref().take(64).collect();
            if entries.is_empty() {
                break;
            }
            service.chunk(&id, chunks, entries).unwrap();
            assert_eq!(service.slots.lock().unwrap()[&id].expires, expires);
            chunks += 1;
        }
        (id, chunks)
    }

    #[tokio::test]
    async fn complete_2148_sessions_reach_exactly_one_query_with_original_options() {
        let service = Service::new();
        let state: HashMap<_, _> = (0..2148)
            .map(|i| (format!("synthetic-session-{i:06}-abcdefghij"), i))
            .collect();
        assert!(serde_json::to_vec(&state).unwrap().len() > 65536);
        let (id, chunks) = upload(&service, state.clone());
        let calls = AtomicUsize::new(0);
        let response = service
            .handle(finish(&id, chunks), |request| async {
                calls.fetch_add(1, Ordering::SeqCst);
                let Request::NewMessages {
                    state: Some(mut actual),
                    limit,
                    with_meta,
                    debug_source,
                } = request
                else {
                    panic!()
                };
                assert_eq!(actual, state);
                assert_eq!(limit, 5);
                assert!(with_meta && !debug_source);
                // 新会话仍由唯一一次原查询处理，上传阶段不补默认游标。
                assert!(!actual.contains_key("new-session"));
                actual.insert("new-session".into(), 5000);
                Response::ok(json!({"new_state":actual,"messages":[],"count":0}))
            })
            .await
            .unwrap();
        assert_eq!(
            response["data"]["new_state"].as_object().unwrap().len(),
            2149
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(service
            .handle(finish(&id, chunks), |_| async { panic!("replayed query") })
            .await
            .is_err());
        assert_eq!(service.quota.available_permits(), MAX_UPLOADS);
    }

    #[tokio::test]
    async fn exact_eight_mib_state_is_accepted_without_truncation() {
        let service = Service::new();
        let mut state = HashMap::new();
        let mut remaining = wire::MAX_STATE_BYTES - 1;
        let mut i = 0;
        while remaining > 0 {
            let length = remaining.min(517) - 5;
            let prefix = format!("{i:08}_");
            let name = format!("{prefix}{}", "x".repeat(length - prefix.len()));
            remaining -= length + 5;
            state.insert(name, 1);
            i += 1;
        }
        assert_eq!(
            serde_json::to_vec(&state).unwrap().len(),
            wire::MAX_STATE_BYTES
        );
        let expected = state.len();
        let (id, chunks) = upload(&service, state);
        service
            .handle(finish(&id, chunks), |request| async move {
                let Request::NewMessages {
                    state: Some(state), ..
                } = request
                else {
                    panic!()
                };
                assert_eq!(state.len(), expected);
                assert_eq!(
                    serde_json::to_vec(&state).unwrap().len(),
                    wire::MAX_STATE_BYTES
                );
                Response::ok(json!({}))
            })
            .await
            .unwrap();
        assert!(service.begin(1, wire::MAX_STATE_BYTES + 1).is_err());
        assert!(service.begin(wire::MAX_SESSIONS + 1, 2).is_err());
    }

    #[tokio::test]
    async fn incomplete_duplicate_or_out_of_order_uploads_never_query() {
        let service = Service::new();
        let state = HashMap::from([("a".into(), 1), ("b".into(), 2)]);
        let id = begin(&service, &state);
        service.chunk(&id, 0, vec![("a".into(), 1)]).unwrap();
        assert!(service
            .handle(finish(&id, 1), |_| async { panic!("partial query") })
            .await
            .is_err());
        let id = begin(&service, &state);
        assert!(service.chunk(&id, 1, vec![("a".into(), 1)]).is_err());
        let id = begin(&service, &state);
        assert!(service
            .chunk(&id, 0, vec![("a".into(), 1), ("a".into(), 1)])
            .is_err());
        let id = begin(&service, &state);
        service.chunk(&id, 0, vec![("a".into(), 1)]).unwrap();
        assert!(service.chunk(&id, 0, vec![("b".into(), 2)]).is_err());
        assert!(service.slots.lock().unwrap().is_empty());
        assert_eq!(service.quota.available_permits(), MAX_UPLOADS);
    }

    #[test]
    fn aggregate_quota_abort_and_account_isolation() {
        let a = Service::new();
        let b = Service::new();
        let id = a.begin(1, 8).unwrap()["id"].as_str().unwrap().to_owned();
        a.begin(1, 8).unwrap();
        assert!(a.begin(1, 8).is_err());
        assert!(b.chunk(&id, 0, vec![("a".into(), 1)]).is_err());
        b.abort(&id);
        assert_eq!(a.slots.lock().unwrap().len(), 2);
        a.abort(&id);
        a.begin(1, 8).unwrap();
        a.clear();
        assert_eq!(a.quota.available_permits(), MAX_UPLOADS);
    }

    #[test]
    fn bad_entries_and_chunk_byte_or_count_limits_release_upload() {
        let service = Service::new();
        for entries in [
            vec![],
            vec![("".into(), 1)],
            vec![("control\n".into(), 1)],
            vec![("x".repeat(513), 1)],
            vec![("x".into(), -1)],
            vec![("x".into(), i64::MAX)],
            (0..1025).map(|i| (i.to_string(), 1)).collect(),
            (0..100)
                .map(|i| (format!("{i:04}{}", "x".repeat(508)), 1))
                .collect(),
        ] {
            let id = service.begin(100_000, wire::MAX_STATE_BYTES).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(service.chunk(&id, 0, entries).is_err());
            assert_eq!(service.quota.available_permits(), MAX_UPLOADS);
        }
    }

    #[tokio::test]
    async fn ttl_reaper_and_shutdown_release_abandoned_uploads() {
        let service = Service::new();
        let id = service.begin(0, 2).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        service.slots.lock().unwrap().get_mut(&id).unwrap().expires = Instant::now();
        let (stop, receiver) = watch::channel(false);
        let worker = tokio::spawn(service.clone().reap(receiver));
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.quota.available_permits() != MAX_UPLOADS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        service.begin(0, 2).unwrap();
        stop.send_replace(true);
        worker.await.unwrap();
        assert!(service.slots.lock().unwrap().is_empty());
        assert_eq!(service.quota.available_permits(), MAX_UPLOADS);
    }

    #[tokio::test]
    async fn query_holds_quota_and_abort_or_dropped_future_releases_it() {
        for stop in ["abort", "drop", "expire"] {
            let service = Service::new();
            let (id, chunks) = upload(&service, HashMap::from([("a".into(), 1)]));
            let (started, ready) = tokio::sync::oneshot::channel();
            let running = service.clone();
            let call = finish(&id, chunks);
            let task = tokio::spawn(async move {
                running
                    .handle(call, |_| async {
                        started.send(()).unwrap();
                        std::future::pending::<Response>().await
                    })
                    .await
            });
            ready.await.unwrap();
            service.begin(0, 2).unwrap();
            assert!(service.begin(0, 2).is_err());
            assert!(service
                .handle(finish(&id, chunks), |_| async { panic!("duplicate query") })
                .await
                .is_err());
            match stop {
                "abort" => {
                    service.abort(&id);
                    assert!(task.await.unwrap().is_err());
                }
                "expire" => {
                    service.slots.lock().unwrap().get_mut(&id).unwrap().expires = Instant::now();
                    service.expire();
                    assert!(task.await.unwrap().is_err());
                }
                _ => {
                    task.abort();
                    assert!(task.await.unwrap_err().is_cancelled());
                }
            }
            assert_eq!(service.quota.available_permits(), 1);
            service.clear();
        }
    }

    #[tokio::test]
    async fn query_error_and_oversize_response_consume_upload_without_leaking_quota() {
        let service = Service::new();
        for response in [
            Response::err("private error must not escape"),
            Response::ok(json!({"text":"x".repeat(2000)})),
        ] {
            let (id, chunks) = upload(&service, HashMap::new());
            let call = wire::Call::Finish {
                id: id.clone(),
                chunks,
                limit: 5,
                with_meta: true,
                debug_source: false,
                max_response_bytes: 1024,
            };
            let result = service.handle(call, |_| async move { response }).await;
            assert!(result.is_err());
            assert_eq!(service.quota.available_permits(), MAX_UPLOADS);
            assert!(!service.slots.lock().unwrap().contains_key(&id));
        }
    }
}
