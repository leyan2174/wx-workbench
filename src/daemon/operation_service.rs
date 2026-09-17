//! Daemon-owned foreground operations, bounded output and client leases.
use crate::{
    runtime::RuntimeContext,
    service::{
        operation_protocol::*,
        protocol::{Call, ServiceError},
    },
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::{watch, Notify, Semaphore},
    task::JoinHandle,
};

const MAX_OPERATIONS: usize = 64;
const MAX_RUNNING: usize = 4;

struct Output {
    chunks: VecDeque<Chunk>,
    bytes: usize,
    next: u64,
    exit: Option<i32>,
    touched: Instant,
}

struct Entry {
    signature: String,
    output: Mutex<Output>,
    changed: Notify,
    cancelled: watch::Sender<bool>,
}

pub(crate) struct Service {
    runtime: RuntimeContext,
    keys: Arc<super::worker_keys::Broker>,
    tasks: Option<Arc<super::tasks::Service>>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    retired: Mutex<VecDeque<(String, String)>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    capacity: Arc<Semaphore>,
    stopping: watch::Sender<bool>,
}

fn failure(code: &'static str, message: &'static str) -> ServiceError {
    ServiceError::new(code, message)
}

impl Service {
    pub fn new(
        runtime: RuntimeContext,
        tasks: Option<Arc<super::tasks::Service>>,
        keys: Arc<super::worker_keys::Broker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            keys,
            tasks,
            entries: Mutex::new(HashMap::new()),
            workers: Mutex::new(Vec::new()),
            retired: Mutex::new(VecDeque::new()),
            capacity: Arc::new(Semaphore::new(MAX_RUNNING)),
            stopping: watch::channel(false).0,
        })
    }

    pub fn handles(call: &Call) -> bool {
        matches!(
            call,
            Call::OperationStart { .. } | Call::OperationPoll { .. } | Call::OperationCancel { .. }
        )
    }

    pub async fn dispatch(
        self: &Arc<Self>,
        call: Call,
    ) -> std::result::Result<Value, ServiceError> {
        match call {
            Call::OperationStart { id, invocation } => self.start(id, *invocation),
            Call::OperationPoll { id, after } => {
                let entry = self.entry(&id)?;
                let notified = entry.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let page = entry.page(after)?;
                if page.chunks.is_empty() && page.exit_code.is_none() {
                    let _ = tokio::time::timeout(Duration::from_secs(1), notified).await;
                }
                serde_json::to_value(entry.page(after)?)
                    .map_err(|_| failure("serialization", "Invalid operation response"))
            }
            Call::OperationCancel { id } => {
                let entry = self.entry(&id)?;
                entry.cancelled.send_replace(true);
                entry.changed.notify_waiters();
                let terminal = entry.output.lock().unwrap().exit.is_some();
                if terminal {
                    let mut entries = self.entries.lock().unwrap();
                    self.retire(id.clone(), entry.signature.clone());
                    entries.remove(&id);
                }
                Ok(json!({"cancelled":true}))
            }
            _ => Err(failure("invalid_operation", "Not an operation request")),
        }
    }

    fn entry(&self, id: &str) -> std::result::Result<Arc<Entry>, ServiceError> {
        self.entries
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| failure("not_found", "Operation not found in this runtime"))
    }

    fn retire(&self, id: String, signature: String) {
        let mut retired = self.retired.lock().unwrap();
        if !retired.iter().any(|(old, _)| *old == id) {
            retired.push_back((id, signature));
            if retired.len() > 1024 {
                retired.pop_front();
            }
        }
    }

    fn start(
        self: &Arc<Self>,
        id: String,
        invocation: Invocation,
    ) -> std::result::Result<Value, ServiceError> {
        // 与停机取走等待列表互斥，避免停止检查之后漏登记新 worker。
        let mut workers = self.workers.lock().unwrap();
        if *self.stopping.borrow() {
            return Err(failure("stopping", "Daemon is stopping"));
        }
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(failure("invalid_id", "Invalid operation identity"));
        }
        invocation
            .validate()
            .map_err(|_| failure("invalid_operation", "Invalid operation context"))?;
        invocation.operation.validate_request().map_err(|_| {
            failure(
                "invalid_operation",
                "Invalid operation arguments or authorization",
            )
        })?;
        let encoded = zeroize::Zeroizing::new(
            serde_json::to_vec(&invocation)
                .map_err(|_| failure("invalid_operation", "Invalid operation"))?,
        );
        let signature = format!("{:x}", Sha256::digest(&*encoded));
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|id, entry| {
            let output = entry.output.lock().unwrap();
            let keep = output.exit.is_none() || output.touched.elapsed() < Duration::from_secs(60);
            if !keep {
                self.retire(id.clone(), entry.signature.clone());
            }
            keep
        });
        if let Some((_, previous)) = self
            .retired
            .lock()
            .unwrap()
            .iter()
            .find(|(old, _)| *old == id)
        {
            return Err(if previous == &signature {
                failure(
                    "consumed",
                    "Operation result has been released; it was not executed again",
                )
            } else {
                failure(
                    "submission_conflict",
                    "Operation identity already belongs to a different request",
                )
            });
        }
        if let Some(entry) = entries.get(&id) {
            if entry.signature != signature {
                return Err(failure(
                    "submission_conflict",
                    "Operation identity already belongs to a different request",
                ));
            }
            return Ok(json!({"id":id}));
        }
        if entries.len() >= MAX_OPERATIONS {
            return Err(failure("busy", "Too many unconsumed operations"));
        }
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure("busy", "Operation workers are busy"))?;
        let entry = Arc::new(Entry {
            signature,
            output: Mutex::new(Output {
                chunks: VecDeque::new(),
                bytes: 0,
                next: 1,
                exit: None,
                touched: Instant::now(),
            }),
            changed: Notify::new(),
            cancelled: watch::channel(false).0,
        });
        entries.insert(id.clone(), entry.clone());
        drop(entries);
        let runtime = self.runtime.clone();
        let shutdown = self.stopping.subscribe();
        let tasks = self.tasks.clone();
        let keys = self.keys.clone();
        let worker = tokio::spawn(async move {
            let _permit = permit;
            execute(runtime, invocation, entry, shutdown, tasks, keys).await;
        });
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
        Ok(json!({"id":id}))
    }

    pub fn idle(&self) -> bool {
        self.capacity.available_permits() == MAX_RUNNING
            && self.entries.lock().unwrap().values().all(|entry| {
                entry.output.lock().unwrap().touched.elapsed() > Duration::from_secs(LEASE_SECS)
            })
    }

    pub async fn shutdown(&self) {
        self.stopping.send_replace(true);
        for entry in self.entries.lock().unwrap().values() {
            entry.cancelled.send_replace(true);
            entry.changed.notify_waiters();
        }
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        for worker in workers {
            let _ = worker.await;
        }
    }
}

impl Entry {
    fn page(&self, after: u64) -> std::result::Result<Page, ServiceError> {
        let mut output = self.output.lock().unwrap();
        if after >= output.next
            || output
                .chunks
                .front()
                .is_some_and(|chunk| chunk.seq > after + 1)
        {
            return Err(failure(
                "invalid_cursor",
                "Operation output cursor is invalid",
            ));
        }
        output.touched = Instant::now();
        while output
            .chunks
            .front()
            .is_some_and(|chunk| chunk.seq <= after)
        {
            output.bytes -= output.chunks.pop_front().unwrap().bytes.len();
        }
        let mut bytes = 0;
        let chunks: Vec<_> = output
            .chunks
            .iter()
            .take_while(|chunk| {
                bytes += chunk.bytes.len();
                bytes <= PAGE_BYTES
            })
            .cloned()
            .collect();
        let last = chunks.last().map_or(after, |chunk| chunk.seq);
        let exit_code = if last + 1 == output.next {
            output.exit
        } else {
            None
        };
        drop(output);
        self.changed.notify_waiters();
        Ok(Page { chunks, exit_code })
    }

    async fn append(&self, bytes: &[u8], stderr: bool) -> Result<()> {
        loop {
            if *self.cancelled.borrow() {
                anyhow::bail!("Operation cancelled");
            }
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut output = self.output.lock().unwrap();
                if output.bytes + bytes.len() <= BUFFER_BYTES {
                    let seq = output.next;
                    output.next += 1;
                    output.bytes += bytes.len();
                    output.chunks.push_back(Chunk {
                        seq,
                        stderr,
                        bytes: bytes.to_vec(),
                    });
                    drop(output);
                    self.changed.notify_waiters();
                    return Ok(());
                }
            }
            let _ = tokio::time::timeout(Duration::from_millis(250), notified).await;
        }
    }

    fn finish(&self, code: i32) {
        self.output.lock().unwrap().exit = Some(code);
        self.changed.notify_waiters();
    }
}

async fn drain(mut input: impl AsyncRead + Unpin, entry: Arc<Entry>, stderr: bool) -> Result<()> {
    let mut bytes = [0u8; CHUNK_BYTES];
    loop {
        let count = input.read(&mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        entry.append(&bytes[..count], stderr).await?;
    }
}

async fn spawn(
    runtime: &RuntimeContext,
    invocation: &Invocation,
    keys: &Arc<super::worker_keys::Broker>,
) -> Result<(
    tokio::process::Child,
    super::tasks::process::Job,
    Option<super::worker_keys::Registration>,
)> {
    use tokio::process::Command;
    let job = if restarts_user_application(&invocation.operation) {
        super::tasks::process::Job::for_account_capture()?
    } else {
        super::tasks::process::Job::new()?
    };
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .env_clear()
        .envs(&invocation.environment.0)
        .current_dir(&invocation.cwd)
        .env("WX_CLI_CONFIG", &runtime.config_path)
        .env("WX_CLI_HOME", &runtime.root)
        .env("WX_DAEMON_OPERATION_WORKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(0x08000004);
    if !runtime.is_bootstrap() {
        command.env("WX_CLI_EXPECTED_RUNTIME", &runtime.id);
    }
    let mut child = command
        .spawn()
        .context("Unable to create operation worker")?;
    let result = async {
        job.attach(&child)?;
        let registered = keys.register(&child, &invocation.operation).await?;
        let (access, registration) = match registered {
            Some((access, registration)) => (Some(access), Some(registration)),
            None => (None, None),
        };
        let bytes =
            zeroize::Zeroizing::new(serde_json::to_vec(&crate::service::worker_keys::Input {
                operation: &invocation.operation,
                access,
            })?);
        ensure!(
            bytes.len() <= crate::service::protocol::MAX_REQUEST_BYTES,
            "Operation frame exceeds limit"
        );
        let mut input = child.stdin.take().context("Missing operation input")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            input.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
            input.write_all(&bytes).await?;
            input.shutdown().await
        })
        .await??;
        Ok::<_, anyhow::Error>(registration)
    }
    .await;
    match result {
        Ok(registration) => Ok((child, job, registration)),
        Err(error) => {
            super::tasks::process::reap(child, job).await?;
            Err(error)
        }
    }
}

fn restarts_user_application(operation: &crate::service::operations::Operation) -> bool {
    matches!(
        operation,
        crate::service::operations::Operation::Initialize {
            force: true,
            provider: crate::scanner::KeyProvider::Account,
            restart: true,
            ..
        }
    )
}

async fn execute(
    runtime: RuntimeContext,
    invocation: Invocation,
    entry: Arc<Entry>,
    mut shutdown: watch::Receiver<bool>,
    tasks: Option<Arc<super::tasks::Service>>,
    keys: Arc<super::worker_keys::Broker>,
) {
    let refresh = invocation.operation.requires_snapshot_reload();
    let spawned = spawn(&runtime, &invocation, &keys).await;
    drop(invocation);
    let (mut child, job, _registration) = match spawned {
        Ok(value) => value,
        Err(error) => {
            let message = error.downcast_ref::<crate::key_store::Error>().map_or_else(
                || "Unable to start daemon operation worker".to_owned(),
                ToString::to_string,
            );
            let _ = entry.append(format!("{message}\n").as_bytes(), true).await;
            entry.finish(1);
            return;
        }
    };
    let mut job = Some(job);
    // Registered key writers publish their generation in the daemon transaction.
    let refresh = refresh && _registration.is_none();
    let mut stdout = tokio::spawn(drain(child.stdout.take().unwrap(), entry.clone(), false));
    let mut stderr = tokio::spawn(drain(child.stderr.take().unwrap(), entry.clone(), true));
    let mut cancel = entry.cancelled.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut exit = None;
    let code = loop {
        tokio::select! {
            status = child.wait(), if exit.is_none() => {
                exit = Some(status.ok().and_then(|status| status.code()).unwrap_or(1));
                // Do not let a descendant retain output pipes after the operation exits.
                drop(job.take());
            },
            _ = shutdown.changed() => break 130,
            _ = cancel.changed() => break 130,
            _ = tick.tick() => {
                if *shutdown.borrow() || *cancel.borrow() || entry.output.lock().unwrap().touched.elapsed() > Duration::from_secs(LEASE_SECS) {
                    break 130;
                }
                if let Some(code) = exit {
                    if stdout.is_finished() && stderr.is_finished() { break code; }
                }
            }
        }
    };
    entry.cancelled.send_replace(true);
    entry.changed.notify_waiters();
    drop(job.take());
    let _ = child.kill().await;
    let _ = child.wait().await;
    // Output readers must be done before the terminal marker can be published.
    if !stdout.is_finished() {
        stdout.abort();
    }
    if !stderr.is_finished() {
        stderr.abort();
    }
    let out = (&mut stdout).await;
    let err = (&mut stderr).await;
    let failed = !matches!(out, Ok(Ok(()))) || !matches!(err, Ok(Ok(())));
    if refresh {
        if let Some(tasks) = tasks {
            tasks.refresh_configuration().await;
        }
    }
    entry.finish(if code == 0 && failed { 1 } else { code });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_authorized_account_capture_releases_application_children() {
        use crate::{scanner::KeyProvider, service::operations::Operation};
        for provider in [
            KeyProvider::Saved,
            KeyProvider::Memory,
            KeyProvider::Account,
        ] {
            for force in [false, true] {
                for restart in [false, true] {
                    let operation = Operation::Initialize {
                        force,
                        provider,
                        restart,
                        db_dir_override: None,
                        executable: None,
                        timeout: 300,
                    };
                    assert_eq!(
                        restarts_user_application(&operation),
                        force && restart && provider == KeyProvider::Account
                    );
                }
            }
        }
        assert!(!restarts_user_application(
            &crate::service::operations::Operation::Capabilities { json: true }
        ));
    }

    fn entry() -> Arc<Entry> {
        Arc::new(Entry {
            signature: "synthetic".into(),
            output: Mutex::new(Output {
                chunks: VecDeque::new(),
                bytes: 0,
                next: 1,
                exit: None,
                touched: Instant::now(),
            }),
            changed: Notify::new(),
            cancelled: watch::channel(false).0,
        })
    }

    #[tokio::test]
    async fn output_is_exact_ordered_repeatable_until_acknowledged() {
        let entry = entry();
        entry.append(&[0, 0xf0, 0x9f], false).await.unwrap();
        entry.append(b"stderr\n", true).await.unwrap();
        let first = entry.page(0).unwrap();
        assert_eq!(first.chunks[0].bytes, [0, 0xf0, 0x9f]);
        assert!(!first.chunks[0].stderr);
        assert_eq!(first.chunks[1].seq, 2);
        assert!(first.chunks[1].stderr);
        assert_eq!(entry.page(0).unwrap().chunks.len(), 2);
        assert_eq!(entry.page(1).unwrap().chunks[0].seq, 2);
        assert!(entry.page(2).unwrap().chunks.is_empty());
        assert_eq!(entry.output.lock().unwrap().bytes, 0);
        assert!(entry.page(3).is_err());
    }

    #[tokio::test]
    async fn bounded_buffer_backpressures_without_dropping_output() {
        let entry = entry();
        let chunk = vec![1; CHUNK_BYTES];
        for _ in 0..BUFFER_BYTES / CHUNK_BYTES {
            entry.append(&chunk, false).await.unwrap();
        }
        assert_eq!(entry.output.lock().unwrap().bytes, BUFFER_BYTES);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), entry.append(b"next", false))
                .await
                .is_err()
        );
        let first = entry.page(0).unwrap();
        assert_eq!(
            first
                .chunks
                .iter()
                .map(|chunk| chunk.bytes.len())
                .sum::<usize>(),
            PAGE_BYTES
        );
        let after = first.chunks.last().unwrap().seq;
        entry.page(after).unwrap();
        entry.append(b"next", false).await.unwrap();
        assert_eq!(
            entry.output.lock().unwrap().chunks.back().unwrap().bytes,
            b"next"
        );
        assert!(entry.page(0).is_err());
    }

    #[tokio::test]
    async fn terminal_code_is_only_visible_with_the_last_output_page() {
        let entry = entry();
        for _ in 0..PAGE_BYTES / CHUNK_BYTES + 1 {
            entry.append(&vec![2; CHUNK_BYTES], false).await.unwrap();
        }
        entry.finish(7);
        let first = entry.page(0).unwrap();
        assert_eq!(first.exit_code, None);
        let second = entry.page(first.chunks.last().unwrap().seq).unwrap();
        assert_eq!(second.chunks.len(), 1);
        assert_eq!(second.exit_code, Some(7));
    }

    #[tokio::test]
    async fn cancellation_unblocks_full_output_and_shutdown_joins_workers() {
        let entry = entry();
        entry.output.lock().unwrap().bytes = BUFFER_BYTES;
        entry.cancelled.send_replace(true);
        assert!(entry.append(b"cancelled", true).await.is_err());
        let root = tempfile::tempdir().unwrap();
        let runtime = RuntimeContext::from_config(
            root.path().join("config.json"),
            crate::config::Config {
                key_store: None,
                db_dir: root.path().join("db"),
                keys_file: root.path().join("keys.json"),
                decrypted_dir: root.path().join("decrypted"),
                wechat_process: String::new(),
            },
            root.path().join("home"),
        )
        .unwrap();
        let keys = super::super::worker_keys::Broker::new(
            runtime.clone(),
            Arc::new(super::super::query_state::QueryState::new(runtime.clone())),
        );
        let service = Service::new(runtime, None, keys);
        service
            .entries
            .lock()
            .unwrap()
            .insert("test".into(), entry.clone());
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = completed.clone();
        let mut stopping = service.stopping.subscribe();
        service
            .workers
            .lock()
            .unwrap()
            .push(tokio::spawn(async move {
                stopping.changed().await.unwrap();
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        service.shutdown().await;
        assert!(*service.stopping.borrow());
        assert!(*entry.cancelled.borrow());
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(service.workers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_requests_are_not_replayed_and_conflicts_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let runtime = RuntimeContext::from_config(
            root.path().join("config.json"),
            crate::config::Config {
                key_store: None,
                db_dir: root.path().join("db"),
                keys_file: root.path().join("keys.json"),
                decrypted_dir: root.path().join("decrypted"),
                wechat_process: String::new(),
            },
            root.path().join("home"),
        )
        .unwrap();
        let keys = super::super::worker_keys::Broker::new(
            runtime.clone(),
            Arc::new(super::super::query_state::QueryState::new(runtime.clone())),
        );
        let service = Service::new(runtime, None, keys);
        let invocation = Invocation {
            operation: crate::service::operations::Operation::Capabilities { json: true },
            cwd: root.path().to_owned(),
            environment: Environment(Default::default()),
        };
        let id = "a".repeat(64);
        let entry = Arc::new(Entry {
            signature: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&invocation).unwrap())
            ),
            output: Mutex::new(Output {
                chunks: VecDeque::new(),
                bytes: 0,
                next: 1,
                exit: Some(0),
                touched: Instant::now(),
            }),
            changed: Notify::new(),
            cancelled: watch::channel(false).0,
        });
        service.entries.lock().unwrap().insert(id.clone(), entry);
        assert!(service.start(id.clone(), invocation.clone()).is_ok());
        assert!(service.workers.lock().unwrap().is_empty());
        let mut changed = invocation.clone();
        changed.environment.0.insert("SCOPE".into(), "other".into());
        assert_eq!(
            service.start(id.clone(), changed).unwrap_err().code,
            "submission_conflict"
        );
        assert!(service.entry(&"b".repeat(64)).is_err());
        service
            .dispatch(Call::OperationCancel { id: id.clone() })
            .await
            .unwrap();
        assert!(service.entries.lock().unwrap().is_empty());
        assert_eq!(
            service.start(id, invocation.clone()).unwrap_err().code,
            "consumed"
        );

        // 持有登记锁时，请求不能越过准入检查；停机后释放锁也不能重新执行。
        let registry = service.workers.lock().unwrap();
        let candidate = service.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let entered = barrier.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let submit = std::thread::spawn(move || {
            entered.wait();
            send.send(candidate.start(String::new(), invocation))
                .unwrap();
        });
        barrier.wait();
        assert!(matches!(
            receive.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        service.stopping.send_replace(true);
        drop(registry);
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap_err()
                .code,
            "stopping"
        );
        submit.join().unwrap();
        assert!(service.entries.lock().unwrap().is_empty());
        assert!(service.workers.lock().unwrap().is_empty());
    }
}
