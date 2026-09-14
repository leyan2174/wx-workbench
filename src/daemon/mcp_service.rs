//! Daemon-owned MCP execution. Call only on a blocking worker, outside query leases.
#[path = "mcp_service/voice.rs"]
pub mod voice;

use crate::{
    ipc::{Request, Response},
    mcp::protocol::{CallBudget, CallContext, CancellationToken, DispatchError},
    runtime::RuntimeContext,
};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
};
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SESSIONS: usize = 64;

#[derive(Default)]
struct Lifecycle {
    stopping: bool,
    next: u64,
    active: HashMap<u64, CancellationToken>,
}

fn lifecycle() -> &'static (Mutex<Lifecycle>, Condvar) {
    static STATE: OnceLock<(Mutex<Lifecycle>, Condvar)> = OnceLock::new();
    STATE.get_or_init(|| (Mutex::new(Lifecycle::default()), Condvar::new()))
}

struct ActiveCall(u64);
impl ActiveCall {
    fn new(context: &CallContext) -> Result<Self, DispatchError> {
        let mut state = lifecycle()
            .0
            .lock()
            .map_err(|_| DispatchError::Unavailable)?;
        if state.stopping {
            return Err(DispatchError::Unavailable);
        }
        let id = state.next;
        state.next = state
            .next
            .checked_add(1)
            .ok_or(DispatchError::Unavailable)?;
        state.active.insert(id, context.cancellation());
        Ok(Self(id))
    }
}
impl Drop for ActiveCall {
    fn drop(&mut self) {
        if let Ok(mut state) = lifecycle().0.lock() {
            state.active.remove(&self.0);
            lifecycle().1.notify_all();
        }
    }
}

/// 释放 daemon 运行时前须等待本函数完成，即使服务处理任务已被中止。
/// 向活动调用发出取消信号，等待其退出并释放资源后清空会话。
/// 取消依赖各执行路径的检查点，不代表所有提交都能阻止，也不会回滚已发布产物。
pub async fn shutdown() -> Result<()> {
    {
        let mut state = lifecycle()
            .0
            .lock()
            .map_err(|_| anyhow!("MCP lifecycle unavailable"))?;
        state.stopping = true;
        for cancellation in state.active.values() {
            cancellation.cancel();
        }
    }
    tokio::task::spawn_blocking(|| -> Result<()> {
        let mut state = lifecycle()
            .0
            .lock()
            .map_err(|_| anyhow!("MCP lifecycle unavailable"))?;
        while !state.active.is_empty() {
            state = lifecycle()
                .1
                .wait(state)
                .map_err(|_| anyhow!("MCP drain failed"))?;
        }
        drop(state);
        sessions()
            .lock()
            .map_err(|_| anyhow!("MCP sessions unavailable"))?
            .clear();
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("MCP drain worker failed"))?
}

/// Constructed from process startup arguments, never from tools/call arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSettings {
    pub media_output_root: Option<PathBuf>,
    pub image_key_file: Option<PathBuf>,
    pub configured_local_python: bool,
    pub voice: voice::Args,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub session: String,
    /// False after the first reply: a daemon restart must not silently rebind a session.
    pub open_session: bool,
    pub owner_pid: u32,
    pub runtime_id: String,
    pub host: HostSettings,
    pub budget: CallBudget,
    /// None closes the session without touching the account or query state.
    pub request: Option<Box<Request>>,
}

pub fn unpack(response: Response) -> Result<Response, DispatchError> {
    if !response.ok || response.error.is_some() {
        return Err(DispatchError::Unavailable);
    }
    serde_json::from_value::<Result<Response, DispatchError>>(response.data)
        .map_err(|_| DispatchError::InvalidResponse)?
}

fn pack(result: Result<Response, DispatchError>) -> Response {
    match serde_json::to_value(result) {
        Ok(value) => Response::ok(value),
        Err(_) => Response::err("MCP service response failed"),
    }
}

struct Owner(std::os::windows::io::OwnedHandle);
impl Owner {
    fn open(pid: u32) -> Result<Self, DispatchError> {
        use std::os::windows::io::FromRawHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map_err(|_| DispatchError::Unavailable)?;
        Ok(Self(unsafe {
            std::os::windows::io::OwnedHandle::from_raw_handle(handle.0)
        }))
    }
    fn alive(&self) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{Foundation::HANDLE, System::Threading::GetExitCodeProcess};
        let mut code = 0;
        unsafe { GetExitCodeProcess(HANDLE(self.0.as_raw_handle()), &mut code) }.is_ok()
            && code == 259
    }
}

struct Entry {
    owner_pid: u32,
    owner: Owner,
    session: Arc<Mutex<Session>>,
}
#[derive(Default)]
struct Session {
    pinned: Option<PinnedAccount>,
    policy: Option<Vec<u8>>,
    invalidated: bool,
}

fn sessions() -> &'static Mutex<HashMap<String, Entry>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    SESSIONS.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some(entries) = SESSIONS.get() {
                if let Ok(mut entries) = entries.lock() {
                    entries.retain(|_, entry| entry.owner.alive());
                }
            }
        });
        Mutex::default()
    })
}

/// Parent dispatches this before acquiring QueryState::snapshot().
/// query must call the in-process query dispatcher, never a pipe client.
pub fn dispatch(
    call: Call,
    runtime: &RuntimeContext,
    mut query: impl FnMut(Request, &CallContext, usize) -> Result<Response, DispatchError>,
) -> Response {
    let result = (|| {
        if call.session.is_empty()
            || call.session.len() > 128
            || call.runtime_id != runtime.id
            || call.owner_pid == 0
        {
            return Err(DispatchError::Unavailable);
        }
        let context = CallContext::from_budget(call.budget.clone())?;
        let _active = ActiveCall::new(&context)?;
        let session = {
            let mut entries = sessions().lock().map_err(|_| DispatchError::Unavailable)?;
            entries.retain(|_, entry| entry.owner.alive());
            if let Some(entry) = entries.get(&call.session) {
                if entry.owner_pid != call.owner_pid {
                    return Err(DispatchError::Unavailable);
                }
            }
            if call.request.is_none() {
                entries.remove(&call.session);
                return Ok(Response::ok(serde_json::json!({"closed":true})));
            }
            if !entries.contains_key(&call.session) {
                if !call.open_session || entries.len() >= MAX_SESSIONS {
                    return Err(DispatchError::Unavailable);
                }
                entries.insert(
                    call.session.clone(),
                    Entry {
                        owner_pid: call.owner_pid,
                        owner: Owner::open(call.owner_pid)?,
                        session: Arc::new(Mutex::new(Session::default())),
                    },
                );
            }
            Arc::clone(&entries[&call.session].session)
        };
        let mut session = session.lock().map_err(|_| DispatchError::Unavailable)?;
        context.check()?;
        session.execute(call, runtime, &context, |request, context, limit| {
            context.check()?;
            let response = query(request, context, limit)?;
            context.check()?;
            // The previous bounded query client rejected transport-level ok:false.
            if !response.ok {
                return Err(DispatchError::Unavailable);
            }
            if serde_json::to_vec(&response)
                .map_err(|_| DispatchError::InvalidResponse)?
                .len()
                > limit
            {
                return Err(DispatchError::ResultLimit);
            }
            Ok(response)
        })
    })();
    pack(result)
}

impl Session {
    fn execute(
        &mut self,
        call: Call,
        runtime: &RuntimeContext,
        context: &CallContext,
        mut query: impl FnMut(Request, &CallContext, usize) -> Result<Response, DispatchError>,
    ) -> Result<Response, DispatchError> {
        context.check()?;
        if self.invalidated {
            return Err(DispatchError::Unavailable);
        }
        let policy = call.host;
        let mut request = *call.request.ok_or(DispatchError::Unavailable)?;
        // An MCP envelope cannot invoke global service, lifecycle, export or recursive calls.
        if !matches!(
            request,
            Request::Sessions { .. }
                | Request::Contacts { .. }
                | Request::History { .. }
                | Request::Search { .. }
                | Request::NewMessages { .. }
                | Request::Attachments { .. }
                | Request::DecodeRefer { .. }
                | Request::DecodeTransfer { .. }
                | Request::DecodeLocation { .. }
                | Request::ContactTags
                | Request::TagMembers { .. }
                | Request::VoiceMessages { .. }
                | Request::DecodeFileMessage { .. }
                | Request::DecodeRecordItem { .. }
                | Request::DecodeImage { .. }
                | Request::DecodeVoice { .. }
                | Request::TranscribeVoice { .. }
        ) {
            return Err(DispatchError::Unavailable);
        }
        let fingerprint = serde_json::to_vec(&policy).map_err(|_| DispatchError::Unavailable)?;
        if self
            .policy
            .as_ref()
            .is_some_and(|previous| previous != &fingerprint)
        {
            self.invalidated = true;
            return Err(DispatchError::Unavailable);
        }
        policy.prepare_request(&mut request)?;
        let mut voice = match &request {
            Request::DecodeVoice { local_id, .. } => Some(policy.voice.prepare(
                voice::Operation::Decode,
                *local_id,
                policy.media_output_root.as_deref(),
                context,
            )?),
            Request::TranscribeVoice { local_id, .. } => Some(if policy.configured_local_python {
                policy.voice.prepare_configured_local(*local_id, context)?
            } else {
                policy.voice.prepare(
                    voice::Operation::Transcribe,
                    *local_id,
                    policy.media_output_root.as_deref(),
                    context,
                )?
            }),
            _ => None,
        };
        if self.pinned.is_none() {
            self.pinned = Some(
                PinnedAccount::open(runtime, call.owner_pid)
                    .map_err(|_| DispatchError::Unavailable)?,
            );
            self.policy = Some(fingerprint);
        }
        let pinned = self.pinned.as_ref().ok_or(DispatchError::Unavailable)?;
        if !same_account(&pinned.context, runtime) || !pinned.is_current() {
            self.invalidated = true;
            return Err(DispatchError::Unavailable);
        }
        let max_response_bytes = call.budget.max_response_bytes;
        if let Some(pending) = voice.take() {
            voice = Some(pending.bind(&pinned.context)?);
        }
        context.check()?;
        if let Request::TranscribeVoice { chat, .. } = &mut request {
            if policy.voice.voice_cache_file.is_some() {
                let current = || {
                    context.check()?;
                    if pinned.is_current() {
                        Ok(())
                    } else {
                        Err(DispatchError::Unavailable)
                    }
                };
                let pending = voice.as_mut().ok_or(DispatchError::Unavailable)?;
                // 精确 username 可在联系人或源音频删除后直接命中；显示名不查历史别名。
                if let Some(response) = pending.try_cached(chat, context, current)? {
                    return Ok(response);
                }
                let resolved = query(Request::ResolveChat { chat: chat.clone() }, context, 8192)
                    .map_err(|_| DispatchError::Unavailable)?;
                current()?;
                if !resolved.ok
                    || resolved.error.is_some()
                    || resolved
                        .data
                        .get("exit_code")
                        .is_some_and(|value| value.as_i64() != Some(0))
                    || resolved
                        .data
                        .get("error")
                        .is_some_and(|value| !value.is_null())
                {
                    return Err(DispatchError::QueryFailed);
                }
                let username = resolved.data["username"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty() && name.len() <= 4096)
                    .ok_or(DispatchError::InvalidResponse)?
                    .to_owned();
                pending.bind_username(username.clone())?;
                if username != *chat {
                    if let Some(response) = pending.try_cached(&username, context, current)? {
                        return Ok(response);
                    }
                }
                *chat = username;
            }
        }
        // Query callback acquires its own short-lived lease; no self IPC.
        let response = query(
            request,
            context,
            if voice.is_some() {
                crate::ipc::MAX_PREPARED_VOICE_RESPONSE_BYTES
            } else {
                max_response_bytes
            },
        );
        if !pinned.is_current() {
            self.invalidated = true;
            return Err(DispatchError::Unavailable);
        }
        context.check()?;
        let response = response.map_err(|_| DispatchError::Unavailable)?;
        match voice {
            Some(voice) => voice.finish(response, &pinned.context, context, || {
                context.check()?;
                if pinned.is_current() {
                    Ok(())
                } else {
                    Err(DispatchError::Unavailable)
                }
            }),
            None => Ok(response),
        }
    }
}

impl HostSettings {
    pub(crate) fn prepare_request(
        &self,
        request: &mut Request,
    ) -> std::result::Result<(), DispatchError> {
        if let Request::DecodeImage {
            output_root,
            image_key_file,
            ..
        } = request
        {
            let root = self
                .media_output_root
                .as_deref()
                .ok_or(DispatchError::Unavailable)?;
            let root = host_path(root)?;
            if !root.is_dir() {
                return Err(DispatchError::Unavailable);
            }
            *output_root = root.to_str().ok_or(DispatchError::Unavailable)?.to_owned();
            *image_key_file = self
                .image_key_file
                .as_deref()
                .map(|path| {
                    host_path(path)?
                        .to_str()
                        .map(str::to_owned)
                        .ok_or(DispatchError::Unavailable)
                })
                .transpose()?;
        }
        Ok(())
    }
}

fn host_path(path: &std::path::Path) -> std::result::Result<PathBuf, DispatchError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(DispatchError::Unavailable);
    }
    std::path::absolute(path).map_err(|_| DispatchError::Unavailable)
}

/// 持有配置读锁直到 stdio 会话结束，阻止普通写入/替换将轮询游标带到另一账号。
/// 文件锁不替代操作系统账户权限；不保证抵御特权进程更换祖先目录联接。
struct PinnedAccount {
    _config_lock: File,
    context: RuntimeContext,
    owner: Owner,
}

impl PinnedAccount {
    fn open(expected: &RuntimeContext, owner_pid: u32) -> Result<Self> {
        let owner = Owner::open(owner_pid).map_err(|_| anyhow!("MCP owner unavailable"))?;
        let path = expected.config_path.clone();
        let mut file = open_config_read_lock(&path)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(anyhow!("MCP account configuration exceeds limit"));
        }
        let config: serde_json::Value = serde_json::from_slice(&bytes)?;
        if config
            .get("db_dir")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|p| p.trim().is_empty())
        {
            return Err(anyhow!("MCP account requires an explicit db_dir"));
        }
        // 验证 db_dir 已明确给出；不调用 Config 的默认目录探测或解密函数。
        let context = RuntimeContext::from_config(
            path.clone(),
            crate::config::load_config_at(&path)?,
            expected.root.clone(),
        )?;
        if !same_account(&context, expected) {
            return Err(anyhow!("MCP account configuration changed"));
        }
        Ok(Self {
            _config_lock: file,
            context,
            owner,
        })
    }

    fn is_current(&self) -> bool {
        self.owner.alive()
            && crate::config::load_config_at(&self.context.config_path)
                .and_then(|config| {
                    RuntimeContext::from_config(
                        self.context.config_path.clone(),
                        config,
                        self.context.root.clone(),
                    )
                })
                .is_ok_and(|current| same_account(&self.context, &current))
    }
}

fn same_account(a: &RuntimeContext, b: &RuntimeContext) -> bool {
    a.id == b.id
        && a.config_path == b.config_path
        && a.root == b.root
        && a.config.db_dir == b.config.db_dir
        && a.config.keys_file == b.config.keys_file
        && a.config.decrypted_dir == b.config.decrypted_dir
        && a.config.wechat_process == b.config.wechat_process
}

fn open_config_read_lock(path: &std::path::Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ：其他读取不受影响，编辑配置须先退出 MCP 会话。
    OpenOptions::new().read(true).share_mode(1).open(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn image_host_policy_is_explicit_and_does_not_create_paths() {
        let root = tempfile::tempdir().unwrap();
        let image = || Request::DecodeImage {
            chat: "peer".into(),
            local_id: 7,
            create_time: 0,
            output_root: "untrusted".into(),
            image_key_file: Some("untrusted-key".into()),
        };
        assert_eq!(
            HostSettings::default().prepare_request(&mut image()),
            Err(DispatchError::Unavailable)
        );
        assert!(HostSettings::default()
            .prepare_request(&mut Request::Ping)
            .is_ok());
        let host = HostSettings {
            media_output_root: Some(root.path().into()),
            image_key_file: Some(root.path().join("key.json")),
            ..HostSettings::default()
        };
        let mut request = image();
        host.prepare_request(&mut request).unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["output_root"], root.path().to_str().unwrap());
        assert_eq!(
            value["image_key_file"],
            root.path().join("key.json").to_str().unwrap()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        for invalid in [
            root.path().join("missing"),
            root.path().join(".."),
            PathBuf::new(),
        ] {
            let host = HostSettings {
                media_output_root: Some(invalid),
                ..HostSettings::default()
            };
            assert_eq!(
                host.prepare_request(&mut image()),
                Err(DispatchError::Unavailable)
            );
        }
    }
    use super::*;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, RuntimeContext, Call) {
        let temp = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            db_dir: temp.path().join("db"),
            keys_file: temp.path().join("keys.json"),
            decrypted_dir: temp.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        let path = temp.path().join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let runtime =
            RuntimeContext::from_config(path, config, temp.path().join("runtime")).unwrap();
        let call = Call {
            session: runtime.id.clone(),
            open_session: true,
            owner_pid: std::process::id(),
            runtime_id: runtime.id.clone(),
            host: HostSettings::default(),
            budget: CallContext::default().budget().unwrap(),
            request: Some(Box::new(Request::ContactTags)),
        };
        (temp, runtime, call)
    }

    fn forbidden(_: Request, _: &CallContext, _: usize) -> Result<Response, DispatchError> {
        panic!("unauthorized query executed")
    }

    #[test]
    fn authorization_precedes_account_open_and_query() {
        let (_temp, runtime, mut call) = fixture();
        std::fs::remove_file(&runtime.config_path).unwrap();
        for request in [
            Request::DecodeImage {
                chat: "peer".into(),
                local_id: 1,
                create_time: 0,
                output_root: "tool-controlled".into(),
                image_key_file: None,
            },
            Request::DecodeVoice {
                chat: "peer".into(),
                local_id: 1,
            },
            Request::TranscribeVoice {
                chat: "peer".into(),
                local_id: 1,
            },
            Request::ReloadConfig,
        ] {
            call.request = Some(Box::new(request));
            let mut session = Session::default();
            assert_eq!(
                session
                    .execute(call.clone(), &runtime, &CallContext::default(), forbidden)
                    .unwrap_err(),
                DispatchError::Unavailable
            );
            assert!(session.pinned.is_none());
        }
    }

    #[test]
    fn missing_established_session_is_not_reopened_after_daemon_restart() {
        let (_temp, runtime, mut call) = fixture();
        call.open_session = false;
        assert_eq!(
            unpack(dispatch(call, &runtime, forbidden)).unwrap_err(),
            DispatchError::Unavailable
        );
        assert!(OpenOptions::new()
            .write(true)
            .open(&runtime.config_path)
            .is_ok());
    }

    #[test]
    fn image_paths_are_replaced_only_by_host_policy() {
        let (temp, _, _) = fixture();
        let host = HostSettings {
            media_output_root: Some(temp.path().into()),
            ..Default::default()
        };
        let mut request = Request::DecodeImage {
            chat: "peer".into(),
            local_id: 1,
            create_time: 0,
            output_root: "tool-controlled".into(),
            image_key_file: Some("tool-key".into()),
        };
        host.prepare_request(&mut request).unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["output_root"], temp.path().to_str().unwrap());
        assert!(value["image_key_file"].is_null());
    }

    #[test]
    fn session_holds_config_lock_and_close_releases_it() {
        let (_temp, runtime, mut call) = fixture();
        let expected = json!({"tags":[], "total_tags":0});
        let result = unpack(dispatch(call.clone(), &runtime, |request, _, _| {
            assert!(matches!(request, Request::ContactTags));
            Ok(Response::ok(expected.clone()))
        }))
        .unwrap();
        assert_eq!(result.data, expected);
        assert!(OpenOptions::new()
            .write(true)
            .open(&runtime.config_path)
            .is_err());
        call.request = None;
        unpack(dispatch(call, &runtime, forbidden)).unwrap();
        assert!(OpenOptions::new()
            .write(true)
            .open(&runtime.config_path)
            .is_ok());
    }

    #[test]
    fn same_session_cannot_change_host_policy_or_runtime_identity() {
        let (_temp, runtime, mut call) = fixture();
        let mut session = Session::default();
        session
            .execute(
                call.clone(),
                &runtime,
                &CallContext::default(),
                |_, _, _| Ok(Response::ok(json!({}))),
            )
            .unwrap();
        call.host.voice.backend.allow_upload = true;
        assert_eq!(
            session
                .execute(call.clone(), &runtime, &CallContext::default(), forbidden)
                .unwrap_err(),
            DispatchError::Unavailable
        );
        call.host.voice.backend.allow_upload = false;
        assert_eq!(
            session
                .execute(call, &runtime, &CallContext::default(), forbidden)
                .unwrap_err(),
            DispatchError::Unavailable
        );
    }

    #[test]
    fn deadline_and_frame_budget_survive_ipc() {
        let (_temp, runtime, mut call) = fixture();
        call.budget.deadline_unix_ms = 0;
        assert_eq!(
            unpack(dispatch(call.clone(), &runtime, forbidden)).unwrap_err(),
            DispatchError::TimedOut
        );
        call.budget = CallContext::default().budget().unwrap();
        call.budget.max_response_bytes = 1024;
        let response = dispatch(call.clone(), &runtime, |_, _, limit| {
            assert_eq!(limit, 1024);
            Ok(Response::ok(json!({"large":"x".repeat(2048)})))
        });
        // Existing query transport failures map to Unavailable, not a leaked raw response.
        assert_eq!(unpack(response).unwrap_err(), DispatchError::Unavailable);
        call.request = None;
        unpack(dispatch(call, &runtime, forbidden)).unwrap();
    }

    #[test]
    fn response_error_categories_and_host_flags_roundtrip() {
        for error in [
            DispatchError::Unavailable,
            DispatchError::Internal,
            DispatchError::Cancelled,
            DispatchError::TimedOut,
            DispatchError::QueryFailed,
            DispatchError::InvalidResponse,
            DispatchError::ResultLimit,
        ] {
            assert_eq!(unpack(pack(Err(error.clone()))).unwrap_err(), error);
        }
        let mut host = HostSettings::default();
        host.voice.backend.allow_upload = true;
        host.voice.backend.backend = crate::daemon::operations::asr::BackendKind::ExplicitOpenAi;
        host.voice.backend.api_key_file = Some(PathBuf::from("explicit-key"));
        let value = serde_json::to_value(&host).unwrap();
        let restored: HostSettings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), value);
        let response = Response::err("synthetic business failure");
        let restored = unpack(pack(Ok(response))).unwrap();
        assert!(!restored.ok);
        assert_eq!(
            restored.error.as_deref(),
            Some("synthetic business failure")
        );
    }

    #[test]
    fn authority_wire_rejects_unknown_fields_at_every_new_boundary() {
        let (_temp, _, call) = fixture();
        let baseline = serde_json::to_value(call).unwrap();
        for field in ["executable", "output_path", "allow_upload", "command"] {
            let mut value = baseline.clone();
            value[field] = json!("injected");
            assert!(
                serde_json::from_value::<Call>(value).is_err(),
                "call: {field}"
            );
            let mut value = baseline.clone();
            value["host"][field] = json!("injected");
            assert!(
                serde_json::from_value::<Call>(value).is_err(),
                "host: {field}"
            );
            let mut value = baseline.clone();
            value["host"]["voice"][field] = json!("injected");
            assert!(
                serde_json::from_value::<Call>(value).is_err(),
                "voice: {field}"
            );
            let mut value = baseline.clone();
            value["budget"][field] = json!("injected");
            assert!(
                serde_json::from_value::<Call>(value).is_err(),
                "budget: {field}"
            );
        }
        assert!(serde_json::from_value::<Call>(baseline).is_ok());
    }

    #[test]
    fn real_json_rpc_id_is_budgeted_before_publication() {
        let mut budget = CallContext::default().budget().unwrap();
        budget.max_response_bytes = 1024;
        budget.response_id = json!("id".repeat(600));
        let context = CallContext::from_budget(budget).unwrap();
        assert_eq!(
            context.check_text_result("small"),
            Err(DispatchError::ResultLimit)
        );
    }

    #[test]
    fn daemon_dispatch_publishes_wav_and_never_returns_prepared_audio() {
        use crate::toolkit::asr::{
            database_media::{DatabaseVoice, VoiceEvidence},
            prepared_audio,
        };
        let (temp, runtime, mut call) = fixture();
        let output = temp.path().join("output");
        std::fs::create_dir(&output).unwrap();
        call.host.media_output_root = Some(output.clone());
        call.request = Some(Box::new(Request::DecodeVoice {
            chat: "voice-test".into(),
            local_id: 42,
        }));
        let result = unpack(dispatch(call.clone(), &runtime, |request, _, limit| {
            assert!(matches!(request, Request::DecodeVoice { local_id: 42, .. }));
            assert_eq!(limit, crate::ipc::MAX_PREPARED_VOICE_RESPONSE_BYTES);
            let audio = DatabaseVoice {
                silk: include_bytes!("../../tests/fixtures/audio/silence.silk").to_vec(),
                evidence: VoiceEvidence {
                    username: "voice-test".into(), message_source: "message/message_0.db".into(),
                    message_table: format!("Msg_{:x}", md5::compute("voice-test")),
                    message_local_id:7, server_id:22, create_time:1700000000,
                    media_source:"message/media_0.db".into(), media_rowid:3,
                    media_chat_name_id:9, media_local_id:42,
                },
            };
            let bytes = prepared_audio::encode(&audio, prepared_audio::Limits {
                max_audio_bytes:16 * 1024 * 1024, max_response_bytes:limit,
            }).unwrap();
            Ok(Response::ok(json!({"prepared_audio":serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()})))
        })).unwrap();
        assert!(result.data["mcp_text"].as_str().unwrap().contains(".wav"));
        assert!(result.data.get("prepared_audio").is_none());
        let files: Vec<_> = std::fs::read_dir(&output).unwrap().collect();
        assert_eq!(files.len(), 1);
        assert!(std::fs::read(files[0].as_ref().unwrap().path())
            .unwrap()
            .starts_with(b"RIFF"));
        call.request = None;
        unpack(dispatch(call, &runtime, forbidden)).unwrap();
    }
}
