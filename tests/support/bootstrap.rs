//! Test-runtime assertions and authenticated account/bootstrap cleanup; never kills processes.
use std::os::windows::{
    ffi::OsStringExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn GetProcessTimes(
        process: *mut std::ffi::c_void,
        created: *mut u64,
        exited: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: *mut std::ffi::c_void,
        flags: u32,
        name: *mut u16,
        length: *mut u32,
    ) -> i32;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    fn GetNamedPipeServerProcessId(pipe: *mut std::ffi::c_void, pid: *mut u32) -> i32;
}

fn verified_process(record: &serde_json::Value) -> anyhow::Result<Option<OwnedHandle>> {
    use anyhow::{ensure, Context};
    let pid = u32::try_from(record["pid"].as_u64().context("missing bootstrap PID")?)?;
    let raw = unsafe { OpenProcess(0x00100000 | 0x1000, 0, pid) };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(87) {
            return Ok(None);
        }
        return Err(error).context("open bootstrap process for query/wait");
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    ensure!(
        unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) } != 0,
        "read bootstrap process birth failed"
    );
    ensure!(
        Some(created) == record["created"].as_u64(),
        "bootstrap PID was reused"
    );
    let mut name = vec![0u16; 32768];
    let mut length = name.len() as u32;
    ensure!(
        unsafe { QueryFullProcessImageNameW(raw, 0, name.as_mut_ptr(), &mut length) } != 0,
        "read bootstrap process executable failed"
    );
    let actual =
        PathBuf::from(std::ffi::OsString::from_wide(&name[..length as usize])).canonicalize()?;
    let recorded = PathBuf::from(
        record["exe"]
            .as_str()
            .context("missing bootstrap executable")?,
    )
    .canonicalize()?;
    let expected = option_env!("CARGO_BIN_EXE_wx")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("WX_SECURITY_WX_EXE").map(PathBuf::from))
        .context("missing test wx executable")?
        .canonicalize()?;
    ensure!(
        actual
            .to_string_lossy()
            .eq_ignore_ascii_case(&recorded.to_string_lossy())
            && actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy()),
        "bootstrap executable is not this test binary"
    );
    Ok(Some(handle))
}

#[track_caller]
#[allow(dead_code)] // Some suites only need lifecycle cleanup.
pub fn assert_only_bootstrap(root: &Path) {
    if !root.exists() {
        return;
    }
    let names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("bootstrap")]);
    assert!(root.join("bootstrap").is_dir());
}

pub struct RuntimeCleanup(pub PathBuf);

#[allow(unused_imports)]
pub use RuntimeCleanup as BootstrapCleanup;

impl RuntimeCleanup {
    fn released(path: &Path) -> std::io::Result<bool> {
        use std::os::windows::fs::OpenOptionsExt;
        match fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(path.join("daemon.lock"))
        {
            Ok(_lock) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) if error.raw_os_error() == Some(32) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn stop(&self) -> anyhow::Result<()> {
        let accounts = self.stop_namespace("accounts");
        let bootstrap = self.stop_namespace("bootstrap");
        accounts.and(bootstrap)
    }

    fn stop_namespace(&self, namespace: &str) -> anyhow::Result<()> {
        use anyhow::{ensure, Context};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = self.0.join(namespace);
        if !directory.exists() {
            return Ok(());
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        for entry in fs::read_dir(directory).context("enumerate test bootstrap runtimes")? {
            let path = entry?.path();
            let pid_file = path.join("daemon.pid");
            if !pid_file.exists() && Self::released(&path)? {
                continue;
            }
            let snapshot = fs::read(&pid_file).context("read bootstrap identity record")?;
            let record: serde_json::Value =
                serde_json::from_slice(&snapshot).context("decode bootstrap identity record")?;
            let id = record["runtime_id"].as_str().unwrap_or("");
            ensure!(
                id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid test runtime identity"
            );
            ensure!(
                path.file_name().is_some_and(|name| name == id),
                "test runtime directory mismatch"
            );
            let Some(process) = verified_process(&record)? else {
                ensure!(
                    Self::released(&path)?,
                    "bootstrap PID vanished but lifetime lock is held"
                );
                continue;
            };
            if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } == 0 {
                continue;
            }
            let shutdown = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    let mut pipe = loop {
                        match tokio::net::windows::named_pipe::ClientOptions::new()
                            .open(format!(r"\\.\pipe\wx-cli-tasks-v1-{id}"))
                        {
                            Ok(pipe) => break pipe,
                            Err(error) if error.raw_os_error() == Some(231) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(error) => {
                                return Err(anyhow::Error::from(error)
                                    .context("connect bootstrap service pipe"))
                            }
                        }
                    };
                    let mut server_pid = 0u32;
                    ensure!(
                        unsafe {
                            GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_pid)
                        } != 0
                            && Some(u64::from(server_pid)) == record["pid"].as_u64(),
                        "bootstrap pipe server PID does not match verified process"
                    );
                    let token = fs::read_to_string(path.join("service-token.key"))
                        .context("read bootstrap service token")?;
                    let request = serde_json::to_vec(&serde_json::json!({
                        "version":1,"runtime_id":id,"token":token,"request":{"op":"shutdown"}
                    }))?;
                    pipe.write_u32_le(request.len() as u32)
                        .await
                        .context("write shutdown frame length")?;
                    pipe.write_all(&request)
                        .await
                        .context("write shutdown frame")?;
                    let length = pipe
                        .read_u32_le()
                        .await
                        .context("read shutdown reply length")?
                        as usize;
                    ensure!(
                        length > 0 && length <= 64 * 1024,
                        "invalid shutdown reply size"
                    );
                    let mut bytes = vec![0; length];
                    pipe.read_exact(&mut bytes)
                        .await
                        .context("read shutdown reply")?;
                    pipe.write_all(&[0])
                        .await
                        .context("acknowledge shutdown reply")?;
                    let reply: serde_json::Value = serde_json::from_slice(&bytes)?;
                    ensure!(
                        reply["version"] == 1 && reply["runtime_id"] == id && reply["ok"] == true,
                        "test bootstrap rejected shutdown"
                    );
                    Ok::<_, anyhow::Error>(())
                })
                .await
            });
            if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != 0 {
                shutdown
                    .context("bootstrap shutdown RPC timed out")?
                    .context("bootstrap shutdown RPC failed")?;
            }
            ensure!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 10_000) } == 0,
                "test bootstrap process did not exit after authenticated shutdown"
            );
            if fs::read(&pid_file).ok().as_deref() == Some(snapshot.as_slice()) {
                fs::remove_file(&pid_file).context("remove unchanged test bootstrap PID record")?;
            }
        }
        Ok(())
    }
}

impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            if thread::panicking() {
                eprintln!("test bootstrap cleanup failed during unwinding: {error:#}");
            } else {
                panic!("test bootstrap cleanup failed: {error:#}");
            }
        }
    }
}
