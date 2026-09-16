//! Authenticated synthetic service server; only query results are injected.
use crate::{ipc::Request, mcp::protocol::DispatchError, mcp_service, runtime::RuntimeContext};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::private_file;

#[allow(
    dead_code,
    reason = "多个独立测试入口共用模拟宿主，各入口只使用所需响应模式"
)]
pub enum Reply {
    Json(Value),
    Raw(Vec<u8>),
    UntilClientCloses(Arc<AtomicBool>),
}

pub struct Mock {
    stop: Option<tokio::sync::watch::Sender<bool>>,
    thread: Option<JoinHandle<Vec<Value>>>,
    mcp_calls: Arc<AtomicUsize>,
    _identity: Identity,
}

struct Identity {
    directory: PathBuf,
    pid: Vec<u8>,
    token: Vec<u8>,
    created_pid: bool,
    created_token: bool,
}

impl Drop for Identity {
    fn drop(&mut self) {
        // A replacement daemon owns its own record; never remove that identity or token.
        if !self.created_pid
            || fs::read(self.directory.join("daemon.pid")).ok().as_deref()
                != Some(self.pid.as_slice())
        {
            return;
        }
        for (name, expected, created) in [
            ("service-token.key", &self.token, self.created_token),
            ("daemon.pid", &self.pid, self.created_pid),
        ] {
            if !created {
                continue;
            }
            let path = self.directory.join(name);
            if fs::read(&path).ok().as_deref() == Some(expected.as_slice()) {
                if let Err(error) = fs::remove_file(path) {
                    if thread::panicking() {
                        eprintln!("mock identity cleanup failed: {error}");
                    } else {
                        panic!("mock identity cleanup failed: {error}");
                    }
                }
            }
        }
    }
}

fn identity(runtime: &RuntimeContext) -> Identity {
    use windows::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    fs::create_dir_all(&runtime.directory).unwrap();
    let mut birth = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut birth,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .unwrap();
    }
    let created = (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime);
    let mut identity = Identity {
        directory: runtime.directory.clone(),
        pid: serde_json::to_vec(&json!({"pid":std::process::id(),
            "exe":std::env::current_exe().unwrap(),"created":created,"runtime_id":runtime.id}))
        .unwrap(),
        token: vec![b'a'; 64],
        created_pid: false,
        created_token: false,
    };
    for (name, bytes) in [
        ("daemon.pid", &identity.pid),
        ("service-token.key", &identity.token),
    ] {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(runtime.directory.join(name))
            .unwrap();
        private_file::restrict(&file).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        if name == "daemon.pid" {
            identity.created_pid = true;
        } else {
            identity.created_token = true;
        }
    }
    identity
}

impl Mock {
    pub fn start(
        runtime: RuntimeContext,
        respond: impl FnMut(&Value) -> Reply + Send + 'static,
    ) -> Self {
        Self::start_with_initial_ping(runtime, None, respond)
    }

    pub fn start_with_initial_ping(
        runtime: RuntimeContext,
        initial_ping: Option<Vec<u8>>,
        respond: impl FnMut(&Value) -> Reply + Send + 'static,
    ) -> Self {
        let identity = identity(&runtime);
        let (ready, wait) = mpsc::channel();
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let mcp_calls = Arc::new(AtomicUsize::new(0));
        let service_calls = Arc::clone(&mcp_calls);
        let thread = thread::spawn(move || {
            use interprocess::local_socket::{
                tokio::prelude::*, GenericNamespaced, ListenerOptions,
            };
            use tokio::{
                io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
                net::windows::named_pipe::ServerOptions,
            };
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                let ping = ListenerOptions::new().name(runtime.pipe_name().to_ns_name::<GenericNamespaced>().unwrap()).create_tokio().unwrap();
                let service_name = format!(r"\\.\pipe\wx-cli-tasks-v1-{}", runtime.id);
                let mut listener = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&service_name).unwrap();
                let requests = Arc::new(Mutex::new(Vec::new()));
                let respond = Arc::new(Mutex::new(respond));
                let initial_ping = Arc::new(Mutex::new(initial_ping));
                let mut connections = tokio::task::JoinSet::new();
                ready.send(()).unwrap();
                loop {
                    tokio::select! {
                        _ = stopped.changed() => break,
                        stream = ping.accept() => {
                            let stream = stream.unwrap();
                            let requests = Arc::clone(&requests);
                            let initial_ping = Arc::clone(&initial_ping);
                            let runtime_id = runtime.id.clone();
                            connections.spawn(async move {
                                let mut reader = tokio::io::BufReader::new(stream);
                                let hello = json!({"version":3,"runtime_id":runtime_id});
                                reader.get_mut().write_all(format!("{hello}\n").as_bytes()).await.unwrap();
                                let mut line = String::new();
                                reader.read_line(&mut line).await.unwrap();
                                let envelope: Value = serde_json::from_str(&line).unwrap();
                                assert_eq!(envelope["version"], 3);
                                assert_eq!(envelope["runtime_id"], runtime_id);
                                let request = envelope["request"].clone();
                                assert_eq!(request["cmd"], "ping", "business bypassed authenticated service");
                                requests.lock().unwrap().push(request);
                                let reply = initial_ping.lock().unwrap().take()
                                    .unwrap_or_else(|| b"{\"ok\":true,\"pong\":true}\n".to_vec());
                                let reply = match serde_json::from_slice::<Value>(&reply) {
                                    Ok(response) => format!("{}\n", json!({"version":3,"runtime_id":runtime_id,"result":"response","response":response})).into_bytes(),
                                    Err(_) => reply,
                                };
                                // Oversized health replies may be rejected while the server writes.
                                let _ = reader.get_mut().write_all(&reply).await;
                            });
                        },
                        result = listener.connect() => {
                            result.unwrap();
                            let next = ServerOptions::new().reject_remote_clients(true).create(&service_name).unwrap();
                            let mut stream = std::mem::replace(&mut listener, next);
                            let runtime = runtime.clone();
                            let respond = Arc::clone(&respond);
                            let requests = Arc::clone(&requests);
                            let service_calls = Arc::clone(&service_calls);
                            connections.spawn(async move {
                                let size = stream.read_u32_le().await.unwrap() as usize;
                                assert!(size <= 64 * 1024);
                                let mut bytes = vec![0;size];
                                stream.read_exact(&mut bytes).await.unwrap();
                                let envelope: Value = serde_json::from_slice(&bytes).unwrap();
                                let authenticated = envelope["version"] == 1 && envelope["runtime_id"] == runtime.id
                                    && envelope["token"] == "a".repeat(64);
                                let silent = Arc::new(Mutex::new(None));
                                let data = if authenticated && envelope["request"]["op"] == "mcp" {
                                    let call: mcp_service::Call = serde_json::from_value(envelope["request"]["request"].clone()).unwrap();
                                    if call.request.is_some() {
                                        service_calls.fetch_add(1, Ordering::SeqCst);
                                    }
                                    let callback_silent = Arc::clone(&silent);
                                    let fixed = runtime.clone();
                                    let response = tokio::task::spawn_blocking(move || {
                                        mcp_service::dispatch(call, &fixed, |request: Request, context, limit| {
                                            let request = serde_json::to_value(request).unwrap();
                                            requests.lock().unwrap().push(request.clone());
                                            let reply = respond.lock().unwrap()(&request);
                                            let bytes = match reply {
                                                Reply::Json(value) => serde_json::to_vec(&value).unwrap(),
                                                Reply::Raw(bytes) => bytes,
                                                Reply::UntilClientCloses(closed) => {
                                                    *callback_silent.lock().unwrap() = Some(closed);
                                                    while context.check().is_ok() {
                                                        std::thread::sleep(Duration::from_millis(5));
                                                    }
                                                    return Err(DispatchError::TimedOut);
                                                },
                                            };
                                            if bytes.len() > limit { return Err(DispatchError::Unavailable); }
                                            serde_json::from_slice(&bytes).map_err(|_| DispatchError::Unavailable)
                                        })
                                    }).await.unwrap();
                                    serde_json::to_value(response).unwrap()
                                } else if authenticated && envelope["request"]["op"] == "info" {
                                    json!({"ready":true})
                                } else {
                                    Value::Null
                                };
                                let closed = silent.lock().unwrap().take();
                                if let Some(closed) = closed {
                                    let mut probe = [0u8;1];
                                    let count = stream.read(&mut probe).await.unwrap();
                                    assert_eq!(count,0,"timed out service call must close without acknowledgement");
                                    closed.store(true,Ordering::Release);
                                    return;
                                }
                                let reply = json!({"version":1,"runtime_id":runtime.id,"ok":authenticated,
                                    "data":data,"error":if authenticated {Value::Null} else {json!({"code":"unauthorized","message":"Service authentication failed"})}});
                                let bytes = serde_json::to_vec(&reply).unwrap();
                                if stream.write_u32_le(bytes.len() as u32).await.is_ok()
                                    && stream.write_all(&bytes).await.is_ok() {
                                    let _ = stream.read_u8().await;
                                }
                            });
                        },
                    }
                    while let Some(result) = connections.try_join_next() { result.unwrap(); }
                }
                // Allow the existing 30-second call budget to finish before closing stalled peers.
                let drained = tokio::time::timeout(Duration::from_secs(45), async {
                    while let Some(result) = connections.join_next().await { result.unwrap(); }
                }).await;
                if drained.is_err() {
                    connections.abort_all();
                    while let Some(result) = connections.join_next().await {
                        if let Err(error) = result {
                            assert!(error.is_cancelled(), "mock connection failed during shutdown: {error}");
                        }
                    }
                    panic!("authenticated mock shutdown exceeded 45 seconds; stalled connections were closed");
                }
                Arc::try_unwrap(requests).unwrap().into_inner().unwrap()
            })
        });
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            stop: Some(stop),
            thread: Some(thread),
            mcp_calls,
            _identity: identity,
        }
    }

    #[allow(dead_code)]
    pub fn mcp_calls(&self) -> usize {
        self.mcp_calls.load(Ordering::SeqCst)
    }

    pub fn finish(mut self) -> Vec<Value> {
        self.stop.take().unwrap().send(true).unwrap();
        let requests = self.thread.take().unwrap().join().unwrap();
        eprintln!("Authenticated mock query requests: {}", json!(requests));
        requests
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                if thread::panicking() {
                    eprintln!("authenticated mock server failed during cleanup");
                } else {
                    panic!("authenticated mock server failed during cleanup");
                }
            }
        }
    }
}
