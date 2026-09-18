//! Overlapped parent stdio with synchronous, non-inheritable child endpoints.
use super::with_user_security;
use anyhow::{Context, Result};
use std::{
    fs::OpenOptions,
    os::windows::io::{AsRawHandle, OwnedHandle},
};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use windows::Win32::{
    Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT},
    Security::Cryptography::{BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
};

pub(super) struct Prepared {
    pub stdin: NamedPipeServer,
    pub stdout: NamedPipeServer,
    pub stderr: NamedPipeServer,
    pub child: [OwnedHandle; 3],
}

fn names() -> Result<[String; 3]> {
    let mut random = [0u8; 32];
    unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut random,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
        .ok()
        .context("Generate stdio pipe identity")?;
    }
    let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(["stdin", "stdout", "stderr"].map(|stream| {
        format!(
            r"\\.\pipe\wx-workbench-stdio-{}-{nonce}-{stream}",
            std::process::id()
        )
    }))
}

fn non_inheritable(handle: &impl AsRawHandle) -> Result<()> {
    unsafe {
        SetHandleInformation(
            HANDLE(handle.as_raw_handle()),
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAGS(0),
        )
    }
    .context("Clear stdio handle inheritance")
}

fn server(name: &str, parent_writes: bool) -> Result<NamedPipeServer> {
    let server = with_user_security(|_, attributes| {
        // The shared helper owns the SID, ACL and descriptor through CreateNamedPipeW.
        unsafe {
            ServerOptions::new()
                .access_inbound(!parent_writes)
                .access_outbound(parent_writes)
                .pipe_mode(PipeMode::Byte)
                .reject_remote_clients(true)
                .first_pipe_instance(true)
                .max_instances(1)
                .create_with_security_attributes_raw(
                    name,
                    (attributes as *mut windows::Win32::Security::SECURITY_ATTRIBUTES).cast(),
                )
        }
        .context("Create overlapped stdio pipe")
    })?;
    non_inheritable(&server)?;
    Ok(server)
}

fn child(name: &str, parent_writes: bool) -> Result<OwnedHandle> {
    // No FILE_FLAG_OVERLAPPED: native child stdio must support synchronous I/O.
    // Open only the already-created instance; never wait/retry a busy pipe.
    let file = OpenOptions::new()
        .read(parent_writes)
        .write(!parent_writes)
        .open(name)
        .context("Open synchronous child stdio")?;
    non_inheritable(&file)?;
    Ok(file.into())
}

async fn pair(name: &str, parent_writes: bool) -> Result<(NamedPipeServer, OwnedHandle)> {
    let server = server(name, parent_writes)?;
    let child = child(name, parent_writes)?;
    server.connect().await.context("Connect stdio pipe")?;
    Ok((server, child))
}

async fn prepare_named(names: &[String; 3]) -> Result<Prepared> {
    let (stdin, child_stdin) = pair(&names[0], true).await?;
    let (stdout, child_stdout) = pair(&names[1], false).await?;
    let (stderr, child_stderr) = pair(&names[2], false).await?;
    Ok(Prepared {
        stdin,
        stdout,
        stderr,
        child: [child_stdin, child_stdout, child_stderr],
    })
}

pub(super) async fn prepare() -> Result<Prepared> {
    prepare_named(&names()?).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{Read, Write},
        mem::size_of,
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        time::{sleep, timeout},
    };
    use windows::Win32::{
        Foundation::{GetHandleInformation, BOOL},
        Security::{
            EqualSid, GetAce, GetKernelObjectSecurity, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, ACCESS_ALLOWED_ACE,
            DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            SE_DACL_PROTECTED,
        },
    };

    const DEADLINE: Duration = Duration::from_secs(5);

    // Narrow test probes without requiring another windows crate feature.
    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "GetNamedPipeInfo"]
        fn get_named_pipe_info(
            handle: HANDLE,
            flags: *mut u32,
            out_buffer: *mut u32,
            in_buffer: *mut u32,
            max_instances: *mut u32,
        ) -> BOOL;
    }

    fn assert_byte_pipe(read_end: &impl AsRawHandle, server_end: bool) {
        let mut flags = 0;
        let mut instances = 0;
        let ok = unsafe {
            get_named_pipe_info(
                HANDLE(read_end.as_raw_handle()),
                &mut flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut instances,
            )
        };
        assert!(ok.as_bool(), "{}", std::io::Error::last_os_error());
        assert_eq!(flags & 4, 0, "PIPE_TYPE_MESSAGE must not be set");
        assert_eq!(flags & 1, u32::from(server_end), "wrong pipe side");
        assert_eq!(instances, 1);
    }

    fn assert_private(handle: &impl AsRawHandle) {
        let mut flags = 0;
        unsafe { GetHandleInformation(HANDLE(handle.as_raw_handle()), &mut flags) }.unwrap();
        assert_eq!(flags & HANDLE_FLAG_INHERIT.0, 0);
    }

    async fn assert_released(name: &str, parent_writes: bool) -> NamedPipeServer {
        // IOCP may deliver the cancelled operation after the owning future drops.
        timeout(DEADLINE, async {
            loop {
                match server(name, parent_writes) {
                    Ok(recreated) => return recreated,
                    Err(error) => {
                        let code = error
                            .downcast_ref::<std::io::Error>()
                            .and_then(std::io::Error::raw_os_error);
                        assert!(
                            matches!(code, Some(5 | 231)),
                            "unexpected release probe error: {error:#}"
                        );
                    }
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("pipe instance leaked")
    }

    #[tokio::test]
    async fn all_handles_are_private_and_servers_are_byte_mode() {
        let pipes = prepare().await.unwrap();
        for pipe in [&pipes.stdin, &pipes.stdout, &pipes.stderr] {
            assert_private(pipe);
        }
        for handle in &pipes.child {
            assert_private(handle);
        }
        // GetNamedPipeInfo needs read access (or WRITE plus READ_ATTRIBUTES).
        // stdin's parent is deliberately write-only; inspect its child read end.
        assert_byte_pipe(&pipes.child[0], false);
        assert_byte_pipe(&pipes.stdout, true);
        assert_byte_pipe(&pipes.stderr, true);
    }

    fn assert_current_user_dacl(handle: &impl AsRawHandle) {
        with_user_security(|sid, _| unsafe {
            let information = (OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION).0;
            let handle = HANDLE(handle.as_raw_handle());
            let mut length = 0;
            let _ = GetKernelObjectSecurity(
                handle,
                information,
                PSECURITY_DESCRIPTOR::default(),
                0,
                &mut length,
            );
            assert!(length > 0);
            let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
            let descriptor = PSECURITY_DESCRIPTOR(storage.as_mut_ptr().cast());
            GetKernelObjectSecurity(handle, information, descriptor, length, &mut length)?;
            let mut owner = PSID::default();
            let mut defaulted = BOOL::default();
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            assert!(!owner.0.is_null());
            EqualSid(owner, sid)?;
            let mut control = 0;
            let mut revision = 0;
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision)?;
            assert_ne!(control & SE_DACL_PROTECTED.0, 0);
            let mut present = BOOL::default();
            let mut acl = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)?;
            assert!(present.as_bool() && !acl.is_null());
            assert_eq!((*acl).AceCount, 1);
            let mut ace = std::ptr::null_mut();
            GetAce(acl, 0, &mut ace)?;
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!((*ace).Header.AceType, 0);
            assert_eq!((*ace).Header.AceFlags, 0);
            assert_eq!((*ace).Mask, 0x001f01ff);
            EqualSid(PSID(std::ptr::addr_of_mut!((*ace).SidStart).cast()), sid)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn endpoints_have_protected_current_user_only_dacl() {
        let pipes = prepare().await.unwrap();
        for pipe in [&pipes.stdin, &pipes.stdout, &pipes.stderr] {
            assert_current_user_dacl(pipe);
        }
        for handle in &pipes.child {
            assert_current_user_dacl(handle);
        }
    }

    #[tokio::test]
    async fn private_frame_and_eof_reach_synchronous_stdin() {
        let Prepared {
            mut stdin,
            stdout,
            stderr,
            child: [input, output, error],
        } = prepare().await.unwrap();
        drop((stdout, stderr, output, error));
        let reader = tokio::task::spawn_blocking(move || {
            let mut file = File::from(input);
            let mut size = [0; 4];
            file.read_exact(&mut size).unwrap();
            assert_eq!(u32::from_le_bytes(size), 65536);
            let mut bytes = vec![0; 65536];
            file.read_exact(&mut bytes).unwrap();
            assert!(bytes.iter().all(|byte| *byte == b'x'));
            assert_eq!(file.read(&mut [0]).unwrap(), 0);
        });
        let written = timeout(DEADLINE, async {
            stdin.write_all(&65536u32.to_le_bytes()).await?;
            stdin.write_all(&vec![b'x'; 65536]).await
        })
        .await;
        // NamedPipeServer::shutdown alone does not close its write handle.
        drop(stdin);
        written.unwrap().unwrap();
        timeout(DEADLINE, reader).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn synchronous_outputs_are_separate_and_end_at_eof() {
        let Prepared {
            stdin,
            mut stdout,
            mut stderr,
            child: [input, output, error],
        } = prepare().await.unwrap();
        drop((stdin, input));
        let out_writer = tokio::task::spawn_blocking(move || {
            File::from(output).write_all(&vec![0xa5; 256 * 1024])
        });
        let err_writer = tokio::task::spawn_blocking(move || {
            File::from(error).write_all(&vec![0x5a; 256 * 1024])
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let drained = timeout(DEADLINE, async {
            tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))
        })
        .await;
        drop((stdout, stderr));
        drained.unwrap().unwrap();
        timeout(DEADLINE, out_writer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        timeout(DEADLINE, err_writer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(out, vec![0xa5; 256 * 1024]);
        assert_eq!(err, vec![0x5a; 256 * 1024]);
    }

    #[tokio::test]
    async fn cancelled_connect_can_be_reused_or_dropped() {
        for parent_writes in [true, false] {
            let names = names().unwrap();
            let pipe = server(&names[0], parent_writes).unwrap();
            assert!(timeout(Duration::from_millis(20), pipe.connect())
                .await
                .is_err());
            let endpoint = child(&names[0], parent_writes).unwrap();
            timeout(DEADLINE, pipe.connect()).await.unwrap().unwrap();
            drop((pipe, endpoint));
            assert_released(&names[0], parent_writes).await;

            let pipe = server(&names[1], parent_writes).unwrap();
            let pending = async move { pipe.connect().await };
            assert!(timeout(Duration::from_millis(20), pending).await.is_err());
            assert_released(&names[1], parent_writes).await;
        }
    }

    #[tokio::test]
    async fn failed_later_pipe_releases_previously_prepared_handles() {
        for _ in 0..8 {
            let names = names().unwrap();
            let occupied = server(&names[2], false).unwrap();
            let error = prepare_named(&names)
                .await
                .err()
                .expect("occupied stderr must fail");
            let code = error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error);
            assert!(
                matches!(code, Some(5 | 231)),
                "unexpected preparation error: {error:#}"
            );
            let stdin = assert_released(&names[0], true).await;
            let mut stdout = assert_released(&names[1], false).await;
            drop(occupied);
            let mut stderr = assert_released(&names[2], false).await;
            // Consume the exact re-created instances. Dropping probes and immediately
            // preparing again would measure the probes' own pending IOCP cleanup.
            let endpoints = [
                child(&names[0], true).unwrap(),
                child(&names[1], false).unwrap(),
                child(&names[2], false).unwrap(),
            ];
            timeout(DEADLINE, async {
                tokio::try_join!(stdin.connect(), stdout.connect(), stderr.connect())
            })
            .await
            .unwrap()
            .unwrap();
            drop(endpoints);
            timeout(DEADLINE, async {
                let mut out = [0];
                let mut err = [0];
                let counts =
                    tokio::try_join!(stdout.read(&mut out), stderr.read(&mut err)).unwrap();
                assert_eq!(counts, (0, 0));
            })
            .await
            .unwrap();
            drop((stdin, stdout, stderr));
            for (index, name) in names.iter().enumerate() {
                drop(assert_released(name, index == 0).await);
            }
        }
    }

    #[tokio::test]
    async fn client_open_failure_releases_server() {
        for parent_writes in [true, false] {
            let names = names().unwrap();
            let failed = (|| -> Result<()> {
                let _pipe = server(&names[0], parent_writes)?;
                let _wrong_direction = child(&names[0], !parent_writes)?;
                anyhow::bail!("wrong-direction open unexpectedly succeeded")
            })();
            let error = failed.unwrap_err();
            assert_eq!(error.to_string(), "Open synchronous child stdio");
            assert_released(&names[0], parent_writes).await;
        }
    }
}
