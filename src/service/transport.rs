//! Local task RPC. One u32-LE-length-prefixed JSON request/reply per connection,
//! followed by a zero-byte acknowledgement from the client before server close.
//! Pipes and tokens have a protected current-user-only DACL. Clients additionally
//! verify the daemon process identity before transmitting credentials.
use super::protocol::{
    Call, Envelope, Reply, ServiceError, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, VERSION,
};
use crate::runtime::RuntimeContext;
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    future::Future,
    io::{Read, Write},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    sync::{watch, Semaphore},
    task::JoinSet,
};
use windows::Win32::{
    Foundation::{BOOL, BOOLEAN, HANDLE},
    Security::*,
    Storage::FileSystem::{
        FileDispositionInfo, FileRenameInfo, GetFileInformationByHandle,
        SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_DISPOSITION_INFO,
        FILE_RENAME_INFO,
    },
};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const READ_SHARE: u32 = 1;
const ALL_SHARE: u32 = 7;
const OPEN_REPARSE: u32 = 0x00200000;
const BACKUP_SEMANTICS: u32 = 0x02000000;
const REPARSE: u32 = 0x400;

// The closure cannot outlive any SID, ACL, or descriptor backing storage.
fn with_user_security<T>(
    action: impl FnOnce(PSID, &mut SECURITY_ATTRIBUTES) -> Result<T>,
) -> Result<T> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut raw = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw)?;
        let _token = OwnedHandle::from_raw_handle(raw.0);
        let mut length = 0;
        let _ = GetTokenInformation(raw, TokenUser, None, 0, &mut length);
        ensure!(length > 0, "cannot obtain task service user SID");
        let mut user = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            raw,
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            length,
            &mut length,
        )?;
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let length = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + GetLengthSid(sid) as usize;
        let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
        let acl = storage.as_mut_ptr().cast::<ACL>();
        InitializeAcl(acl, length as u32, ACL_REVISION)?;
        AddAccessAllowedAce(acl, ACL_REVISION, 0x001f01ff, sid)?;
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor = PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        InitializeSecurityDescriptor(descriptor, 1)?;
        SetSecurityDescriptorOwner(descriptor, sid, false)?;
        SetSecurityDescriptorDacl(descriptor, true, Some(acl), false)?;
        SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: BOOL(0),
        };
        action(sid, &mut attributes)
    }
}

fn create_pipe(name: &str, first: bool) -> Result<NamedPipeServer> {
    with_user_security(|_, attributes| {
        // Tokio only borrows the descriptor during CreateNamedPipeW.
        Ok(unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    name,
                    (attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )?
        })
    })
}

fn verify_private_security(handle: HANDLE) -> Result<()> {
    with_user_security(|sid, _| unsafe {
        let information = (OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION).0;
        let mut length = 0;
        let _ = GetKernelObjectSecurity(
            handle,
            information,
            PSECURITY_DESCRIPTOR::default(),
            0,
            &mut length,
        );
        ensure!(length > 0, "cannot inspect task identity security");
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let descriptor = PSECURITY_DESCRIPTOR(storage.as_mut_ptr().cast());
        GetKernelObjectSecurity(handle, information, descriptor, length, &mut length)?;
        let mut owner = PSID::default();
        let mut defaulted = BOOL::default();
        GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
        ensure!(!owner.0.is_null(), "task identity has no owner");
        EqualSid(owner, sid).context("task identity belongs to another user")?;
        let mut control = 0u16;
        let mut revision = 0;
        GetSecurityDescriptorControl(descriptor, &mut control, &mut revision)?;
        ensure!(
            control & SE_DACL_PROTECTED.0 != 0,
            "task identity ACL is inherited"
        );
        let mut present = BOOL::default();
        let mut acl = std::ptr::null_mut();
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)?;
        ensure!(
            present.as_bool() && !acl.is_null() && (*acl).AceCount == 1,
            "task identity ACL is not private"
        );
        let mut ace = std::ptr::null_mut();
        GetAce(acl, 0, &mut ace)?;
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        ensure!(
            (*ace).Header.AceType == 0 && (*ace).Header.AceFlags == 0 && (*ace).Mask == 0x001f01ff,
            "unexpected task identity ACL"
        );
        EqualSid(PSID(std::ptr::addr_of_mut!((*ace).SidStart).cast()), sid)
            .context("task identity ACL grants another user access")?;
        Ok(())
    })
}

fn delete_owned(file: &File) -> Result<()> {
    let information = FILE_DISPOSITION_INFO {
        DeleteFile: BOOLEAN(1),
    };
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&information as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )?;
    }
    Ok(())
}

// Never perform path-based cleanup: the name may have been reused after failure.
struct OwnedToken(File);
impl Drop for OwnedToken {
    fn drop(&mut self) {
        let _ = delete_owned(&self.0);
    }
}

fn publish_owned(file: &File, destination: &Path) -> Result<()> {
    let wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let length = offset_of!(FILE_RENAME_INFO, FileName) + wide.len() * size_of::<u16>();
    let mut storage = vec![
        0usize;
        length
            .max(size_of::<FILE_RENAME_INFO>())
            .div_ceil(size_of::<usize>())
    ];
    unsafe {
        let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // Zeroed ReplaceIfExists is false: a racing destination is never replaced.
        (*information).FileNameLength = u32::try_from((wide.len() - 1) * size_of::<u16>())?;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            wide.len(),
        );
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfo,
            information.cast(),
            u32::try_from(length)?,
        )?;
    }
    Ok(())
}

fn stale_token(path: &Path) -> Result<Option<File>> {
    let mut file = match OpenOptions::new()
        .access_mode(0x80010000)
        .share_mode(READ_SHARE)
        .custom_flags(OPEN_REPARSE)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("cannot lock stale task token"),
    };
    regular_file(&file)?;
    verify_private_security(HANDLE(file.as_raw_handle()))?;
    let mut bytes = Zeroizing::new(Vec::new());
    (&mut file).take(65).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() == 64 && bytes.iter().all(|b| b.is_ascii_hexdigit()),
        "unexpected existing task token file"
    );
    Ok(Some(file))
}

pub(crate) fn pipe_name(runtime: &RuntimeContext) -> Result<String> {
    ensure!(
        !runtime.id.is_empty()
            && runtime.id.len() <= 128
            && runtime
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid task runtime id"
    );
    Ok(format!(r"\\.\pipe\wx-cli-tasks-v1-{}", runtime.id))
}

// Pin each directory component against rename/deletion, not just the leaf.
pub(crate) struct DirectoryGuard {
    pub(crate) path: PathBuf,
    _handles: Vec<File>,
}

impl DirectoryGuard {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "task runtime directory must be absolute"
        );
        ensure!(
            !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "invalid task runtime directory"
        );
        let mut handles = Vec::new();
        for parent in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let file = OpenOptions::new()
                .access_mode(0)
                .share_mode(3)
                .custom_flags(OPEN_REPARSE | BACKUP_SEMANTICS)
                .open(parent)
                .context("cannot pin task runtime directory")?;
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_dir() && metadata.file_attributes() & REPARSE == 0,
                "task runtime directory contains a reparse point"
            );
            handles.push(file);
        }
        Ok(Self {
            path: path.canonicalize()?,
            _handles: handles,
        })
    }
}

pub(crate) fn regular_file(file: &File) -> Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(
        file.metadata()?.is_file()
            && info.dwFileAttributes & REPARSE == 0
            && info.nNumberOfLinks == 1,
        "task identity file must be a regular unlinked file"
    );
    Ok(())
}

pub(crate) fn read_identity(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(ALL_SHARE)
        .custom_flags(OPEN_REPARSE)
        .open(path)
        .context("cannot open task identity file")?;
    regular_file(&file)?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "task identity file exceeds limit");
    Ok(bytes)
}

// No Debug implementation: this object owns a credential. Forced process death
// may leave the file; orderly Drop deletes only the still-pinned owned object.
struct TokenFile {
    value: Zeroizing<String>,
    _file: OwnedToken,
    _directory: DirectoryGuard,
}

impl TokenFile {
    fn create(directory: DirectoryGuard, _bound_listener: &NamedPipeServer) -> Result<Self> {
        let path = directory.path.join("service-token.key");
        let stale = stale_token(&path)?;
        // Custom creation preserves deny-write/delete sharing across staging,
        // ACL setup, publication, and the complete daemon lifetime.
        let staged: tempfile::NamedTempFile = tempfile::Builder::new()
            .prefix(".service-token-")
            .disable_cleanup(true)
            .make_in(&directory.path, |path| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .access_mode(0xC00D0000)
                    .share_mode(READ_SHARE)
                    .custom_flags(OPEN_REPARSE)
                    .open(path)
            })?;
        let (file, _untracked_path) = staged.into_parts();
        let mut file = OwnedToken(file);
        regular_file(&file.0)?;
        crate::toolkit::private_file::restrict(&file.0)?;
        with_user_security(|_, attributes| unsafe {
            SetKernelObjectSecurity(
                HANDLE(file.0.as_raw_handle()),
                OWNER_SECURITY_INFORMATION,
                PSECURITY_DESCRIPTOR(attributes.lpSecurityDescriptor),
            )?;
            Ok(())
        })?;
        verify_private_security(HANDLE(file.0.as_raw_handle()))?;
        use windows::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        let mut bytes = Zeroizing::new([0u8; 32]);
        unsafe {
            BCryptGenRandom(
                BCRYPT_ALG_HANDLE::default(),
                &mut *bytes,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
            .ok()?;
        }
        let value = Zeroizing::new(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
        file.0.write_all(value.as_bytes())?;
        file.0.sync_all()?;
        if let Some(stale) = stale {
            delete_owned(&stale)?;
            drop(stale);
        }
        publish_owned(&file.0, &path)
            .context("cannot publish task token without overwriting another file")?;
        Ok(Self {
            value,
            _file: file,
            _directory: directory,
        })
    }
}

pub(crate) async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let size = reader.read_u32_le().await? as usize;
    ensure!(size > 0 && size <= limit, "task frame exceeds size limit");
    let mut bytes = Zeroizing::new(vec![0; size]);
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

pub(crate) fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    struct Bounded {
        bytes: Zeroizing<Vec<u8>>,
        limit: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("task frame exceeds size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Zeroizing::new(Vec::new()),
        limit,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| anyhow::anyhow!("cannot encode task frame within limit"))?;
    Ok(writer.bytes)
}

pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer.write_u32_le(u32::try_from(bytes.len())?).await?;
    writer.write_all(bytes).await?;
    Ok(())
}

fn error(code: &str, message: &str) -> ServiceError {
    ServiceError {
        code: code.into(),
        message: message.into(),
    }
}

fn authenticate(
    version: u32,
    runtime: &str,
    supplied: &str,
    expected_runtime: &str,
    expected: &str,
) -> std::result::Result<(), ServiceError> {
    if version != VERSION {
        return Err(error(
            "unsupported_version",
            "unsupported task protocol version",
        ));
    }
    if runtime != expected_runtime {
        return Err(error("wrong_runtime", "task runtime mismatch"));
    }
    let equal = supplied.len() == expected.len()
        && supplied
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0;
    if !equal {
        return Err(ServiceError::unauthorized());
    }
    Ok(())
}

/// Runs inside the existing daemon, without startup or web coupling.
/// The caller must hold its daemon.lock for this entire future. The first pipe
/// instance is acquired before any token mutation, also excluding rival servers.
/// The service validates event long-poll limits. Submissions require idempotency
/// keys: a timeout/disconnect does not prove non-execution.
pub(crate) async fn serve<F, Fut>(
    runtime: RuntimeContext,
    handler: Arc<F>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()>
where
    F: Fn(Call) -> Fut + Send + Sync + 'static + ?Sized,
    Fut: Future<Output = std::result::Result<Value, ServiceError>> + Send + 'static,
{
    if *shutdown.borrow() {
        return Ok(());
    }
    let name = pipe_name(&runtime)?;
    let directory = DirectoryGuard::open(&runtime.directory)?;
    let mut listener = create_pipe(&name, true)?;
    let token = Arc::new(TokenFile::create(directory, &listener)?);
    let slots = Arc::new(Semaphore::new(32));
    let mut connections = JoinSet::new();
    let result: Result<()> = async {
        loop {
            if *shutdown.borrow() { break; }
            let permit = tokio::select! { biased;
                _ = shutdown.changed() => { if *shutdown.borrow() || shutdown.has_changed().is_err() { break; } else { continue; } }
                permit = slots.clone().acquire_owned() => permit?,
            };
            tokio::select! { biased;
                _ = shutdown.changed() => { if *shutdown.borrow() || shutdown.has_changed().is_err() { break; } else { continue; } }
                connected = listener.connect() => connected?,
            }
            // Retain a pipe instance at all times to prevent name takeover.
            let next = create_pipe(&name, false)?;
            let mut stream = std::mem::replace(&mut listener, next);
            let token = token.clone();
            let handler = handler.clone();
            let runtime_id = runtime.id.clone();
            connections.spawn(async move {
                let _permit = permit;
                let _ = async {
                    let bytes = Zeroizing::new(tokio::time::timeout(CALL_TIMEOUT, read_frame(&mut stream, MAX_REQUEST_BYTES)).await??);
                    let mut envelope: Envelope = match serde_json::from_slice(&bytes) {
                        Ok(envelope) => envelope,
                        Err(_) => {
                            let reply = Reply::failure(runtime_id.clone(), error("invalid_request", "Unsupported or invalid service request"));
                            let encoded = encode(&reply, MAX_RESPONSE_BYTES)?;
                            tokio::time::timeout(CALL_TIMEOUT, async {
                                write_frame(&mut stream, &encoded).await?;
                                ensure!(stream.read_u8().await? == 0, "invalid task response acknowledgement");
                                Ok::<(), anyhow::Error>(())
                            }).await??;
                            return Ok::<(), anyhow::Error>(());
                        }
                    };
                    let auth = authenticate(envelope.version, &envelope.runtime_id, &envelope.token, &runtime_id, &token.value);
                    envelope.token.zeroize();
                    let max_response_bytes = envelope.request.response_limit();
                    let outcome = match auth {
                        Ok(()) => tokio::time::timeout(Duration::from_secs(60), handler(envelope.request)).await
                            .unwrap_or_else(|_| Err(error("deadline", "Operation deadline exceeded; outcome may be unknown"))),
                        Err(error) => Err(error),
                    };
                    let reply = match outcome {
                        Ok(data) => Reply::success(runtime_id.clone(), data),
                        Err(error) => Reply::failure(runtime_id.clone(), error),
                    };
                    let bytes = match encode(&reply, max_response_bytes) {
                        Ok(bytes) => bytes,
                        Err(_) => encode(&Reply::failure(runtime_id, error("response_too_large", "task response exceeds size limit")), MAX_RESPONSE_BYTES)?,
                    };
                    tokio::time::timeout(CALL_TIMEOUT, async {
                        write_frame(&mut stream, &bytes).await?;
                        ensure!(stream.read_u8().await? == 0, "invalid task response acknowledgement");
                        Ok::<(), anyhow::Error>(())
                    }).await??;
                    Ok::<(), anyhow::Error>(())
                }.await;
            });
            while connections.try_join_next().is_some() {}
        }
        Ok(())
    }.await;
    // Shutdown can originate in a still-active RPC. Keep both the pipe name and
    // credential alive until that connection sends its reply and receives ACK.
    let _ = tokio::time::timeout(CALL_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    drop(token);
    drop(listener);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_name(directory: &Path) -> String {
        format!(
            r"\\.\pipe\wx-cli-tasks-v1-test-{}-{}",
            std::process::id(),
            directory.file_name().unwrap().to_string_lossy()
        )
    }

    fn leave_stale_token(path: &Path, contents: &[u8]) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(0xC00D0000)
            .open(path)
            .unwrap();
        crate::toolkit::private_file::restrict(&file).unwrap();
        with_user_security(|_, attributes| unsafe {
            SetKernelObjectSecurity(
                HANDLE(file.as_raw_handle()),
                OWNER_SECURITY_INFORMATION,
                PSECURITY_DESCRIPTOR(attributes.lpSecurityDescriptor),
            )?;
            Ok(())
        })
        .unwrap();
        file.write_all(contents).unwrap();
        file.sync_all().unwrap();
    }

    #[tokio::test]
    async fn shutdown_rpc_drains_reply_and_ack_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = RuntimeContext {
            config: crate::config::Config {
                db_dir: PathBuf::new(),
                keys_file: PathBuf::new(),
                decrypted_dir: PathBuf::new(),
                wechat_process: String::new(),
            },
            config_path: PathBuf::new(),
            root: dir.path().into(),
            directory: dir.path().into(),
            id: format!(
                "shutdown-test-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ),
        };
        let name = pipe_name(&runtime).unwrap();
        let runtime_id = runtime.id.clone();
        let path = dir.path().join("service-token.key");
        let (shutdown, receiver) = watch::channel(false);
        let handler = Arc::new(move |call| {
            assert!(matches!(call, Call::Shutdown {}));
            shutdown.send(true).unwrap();
            async { Ok(Value::Null) }
        });
        let serving = tokio::spawn(serve(runtime, handler, receiver));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.exists() {
                assert!(
                    !serving.is_finished(),
                    "service exited before publishing token"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let mut pipe = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(name)
            .unwrap();
        let bytes = read_identity(&path, 64).unwrap();
        let mut envelope = Envelope {
            version: VERSION,
            runtime_id,
            token: String::from_utf8(bytes.to_vec()).unwrap(),
            request: Call::Shutdown {},
        };
        let request = encode(&envelope, MAX_REQUEST_BYTES).unwrap();
        envelope.token.zeroize();
        write_frame(&mut pipe, &request).await.unwrap();
        let bytes = tokio::time::timeout(
            Duration::from_secs(2),
            read_frame(&mut pipe, MAX_RESPONSE_BYTES),
        )
        .await
        .unwrap()
        .unwrap();
        let reply: Reply = serde_json::from_slice(&bytes).unwrap();
        assert!(reply.ok);
        tokio::task::yield_now().await;
        assert!(
            !serving.is_finished(),
            "shutdown must wait for response acknowledgement"
        );
        pipe.write_u8(0).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn every_pipe_instance_has_current_user_only_acl() {
        let dir = tempfile::tempdir().unwrap();
        let name = test_name(dir.path());
        let first = create_pipe(&name, true).unwrap();
        verify_private_security(HANDLE(first.as_raw_handle())).unwrap();
        let next = create_pipe(&name, false).unwrap();
        verify_private_security(HANDLE(next.as_raw_handle())).unwrap();
        assert!(create_pipe(&name, true).is_err());
    }

    #[tokio::test]
    async fn restart_rotates_private_stale_token_after_binding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token.key");
        let old = Zeroizing::new("a".repeat(64));
        leave_stale_token(&path, old.as_bytes());
        let name = test_name(dir.path());
        let listener = create_pipe(&name, true).unwrap();
        let token =
            TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).unwrap();
        assert!(*token.value != *old);
        crate::toolkit::private_file::assert_private_acl(&path);
        assert!(create_pipe(&name, true).is_err());
        assert!(read_identity(&path, 64).unwrap().as_slice() == token.value.as_bytes());
        drop(token);
        assert!(!path.exists());
        drop(listener);
        let restarted = create_pipe(&name, true).unwrap();
        let token =
            TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &restarted).unwrap();
        assert_eq!(token.value.len(), 64);
    }

    #[tokio::test]
    async fn unexpected_regular_file_survives_restart_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token.key");
        leave_stale_token(&path, b"not a task credential");
        let listener = create_pipe(&test_name(dir.path()), true).unwrap();
        assert!(TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a task credential");
    }

    #[tokio::test]
    async fn token_shaped_file_with_untrusted_acl_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token.key");
        let bytes = vec![b'a'; 64];
        std::fs::write(&path, &bytes).unwrap();
        let listener = create_pipe(&test_name(dir.path()), true).unwrap();
        assert!(TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn symbolic_link_is_not_followed_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        let path = dir.path().join("service-token.key");
        leave_stale_token(&original, &[b'a'; 64]);
        if let Err(error) = std::os::windows::fs::symlink_file(&original, &path) {
            if error.raw_os_error() == Some(1314) {
                eprintln!("symlink test skipped: Windows symlink privilege is unavailable");
                return;
            }
            panic!("cannot create symlink fixture: {error}");
        }
        let listener = create_pipe(&test_name(dir.path()), true).unwrap();
        assert!(TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).is_err());
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&original).unwrap(), vec![b'a'; 64]);
    }

    #[test]
    fn failed_noclobber_publish_cleans_only_its_own_object() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        let target = dir.path().join("service-token.key");
        std::fs::write(&target, b"replacement must survive").unwrap();
        let file = OwnedToken(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .access_mode(0xC0010000)
                .share_mode(READ_SHARE)
                .open(&original)
                .unwrap(),
        );
        assert!(publish_owned(&file.0, &target).is_err());
        drop(file);
        assert!(!original.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement must survive");
    }

    #[tokio::test]
    async fn oversized_frame_rejected_without_body_or_eof() {
        let (mut tx, mut rx) = tokio::io::duplex(8);
        tx.write_u32_le(MAX_REQUEST_BYTES as u32 + 1).await.unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(100),
            read_frame(&mut rx, MAX_REQUEST_BYTES)
        )
        .await
        .unwrap()
        .is_err());
    }

    #[tokio::test]
    async fn frame_round_trip_and_truncation() {
        let (mut tx, mut rx) = tokio::io::duplex(32);
        write_frame(&mut tx, b"{}").await.unwrap();
        assert_eq!(&**read_frame(&mut rx, 2).await.unwrap(), b"{}");
        tx.write_u32_le(2).await.unwrap();
        tx.write_all(b"{").await.unwrap();
        drop(tx);
        assert!(read_frame(&mut rx, 2).await.is_err());
    }

    #[test]
    fn authentication_checks_precede_dispatch() {
        assert!(authenticate(VERSION, "a", "secret", "a", "secret").is_ok());
        assert_eq!(
            authenticate(VERSION + 1, "a", "secret", "a", "secret")
                .err()
                .unwrap()
                .code,
            "unsupported_version"
        );
        assert_eq!(
            authenticate(VERSION, "b", "secret", "a", "secret")
                .err()
                .unwrap()
                .code,
            "wrong_runtime"
        );
        assert_eq!(
            authenticate(VERSION, "a", "bad", "a", "secret")
                .err()
                .unwrap()
                .code,
            "unauthorized"
        );
        assert!(encode(&"too large", 2).is_err());
    }

    #[tokio::test]
    async fn token_is_private_locked_rotated_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token.key");
        let listener = create_pipe(&test_name(dir.path()), true).unwrap();
        let token =
            TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).unwrap();
        crate::toolkit::private_file::assert_private_acl(&path);
        assert_eq!(read_identity(&path, 64).unwrap().len(), 64);
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).is_err());
        let old = token.value.clone();
        drop(token);
        assert!(!path.exists());
        let next = TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).unwrap();
        assert!(*old != *next.value);
    }

    #[tokio::test]
    async fn existing_and_hardlinked_tokens_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        let path = dir.path().join("service-token.key");
        leave_stale_token(&original, &[b'a'; 64]);
        std::fs::hard_link(&original, &path).unwrap();
        let listener = create_pipe(&test_name(dir.path()), true).unwrap();
        assert!(TokenFile::create(DirectoryGuard::open(dir.path()).unwrap(), &listener).is_err());
        assert!(read_identity(&path, 64).is_err());
        assert_eq!(std::fs::read(&original).unwrap(), vec![b'a'; 64]);
    }
}
