//! Production creation-time Job ownership and the legacy crash-window contrast.
use super::Job;
use anyhow::{ensure, Context, Result};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            DuplicateHandle, GetHandleInformation, BOOL, DUPLICATE_SAME_ACCESS, FILETIME, HANDLE,
            HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::{JobObjects::*, Threading::*},
    },
};

const FIXTURE: &str = "windows_process::managed::job_assignment::synthetic_process";

struct Attributes {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new() -> Result<Self> {
        let mut bytes = 0;
        let _ = unsafe {
            InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                2,
                0,
                &mut bytes,
            )
        };
        ensure!(bytes > 0, "Attribute-list sizing failed");
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(list, 2, 0, &mut bytes)?;
        }
        Ok(Self {
            _storage: storage,
            list,
        })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

enum Created {
    Native(super::native::Process),
    Legacy {
        process: OwnedHandle,
        thread: OwnedHandle,
        pid: u32,
    },
}
impl Created {
    fn handle(&self) -> HANDLE {
        match self {
            Self::Native(process) => HANDLE(process.handle().as_raw_handle()),
            Self::Legacy { process, .. } => HANDLE(process.as_raw_handle()),
        }
    }
    fn id(&self) -> u32 {
        match self {
            Self::Native(process) => process.id(),
            Self::Legacy { pid, .. } => *pid,
        }
    }
    fn resume(&self) -> Result<()> {
        let thread = match self {
            Self::Native(process) => return process.resume(),
            Self::Legacy { thread, .. } => thread,
        };
        ensure!(
            unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } != u32::MAX,
            "Resume failed"
        );
        Ok(())
    }
    fn created(&self) -> Result<u64> {
        creation_time(self.handle())
    }
}
impl Drop for Created {
    fn drop(&mut self) {
        // Test ownership only; the Job independently owns the process tree.
        unsafe {
            let _ = TerminateProcess(self.handle(), 1);
            let _ = WaitForSingleObject(self.handle(), 5000);
        }
    }
}
fn creation_time(handle: HANDLE) -> Result<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user)?;
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    ensure!(!result.contains(&0), "Embedded NUL");
    result.push(0);
    Ok(result)
}
fn quoted(value: &OsStr) -> Result<Vec<u16>> {
    let raw = wide(value)?;
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in &raw[..raw.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}
fn environment(root: &Path, mode: &str) -> Vec<(OsString, OsString)> {
    let mut env: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| {
            !key.encode_wide()
                .take("WX_JOB_LIST_SPIKE_".len())
                .map(|unit| {
                    if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
                        unit - 32
                    } else {
                        unit
                    }
                })
                .eq("WX_JOB_LIST_SPIKE_".encode_utf16())
        })
        .collect();
    env.push(("WX_JOB_LIST_SPIKE_ROOT".into(), root.as_os_str().into()));
    env.push(("WX_JOB_LIST_SPIKE_MODE".into(), mode.into()));
    env.push((
        "WX_JOB_LIST_SPIKE_VALUE".into(),
        "synthetic value = preserved".into(),
    ));
    env.sort_by_key(|(key, _)| key.to_string_lossy().to_ascii_uppercase());
    env
}
fn duplicate_inheritable(source: &File) -> Result<OwnedHandle> {
    let mut handle = HANDLE::default();
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(source.as_raw_handle()),
            GetCurrentProcess(),
            &mut handle,
            0,
            true,
            DUPLICATE_SAME_ACCESS,
        )?;
        Ok(OwnedHandle::from_raw_handle(handle.0))
    }
}

fn spawn_suspended(job: &Job, root: &Path, mode: &str, stdio: [&File; 3]) -> Result<Created> {
    let mut flags = 0;
    unsafe {
        GetHandleInformation(job.0, &mut flags)?;
    }
    ensure!(
        flags & HANDLE_FLAG_INHERIT.0 == 0,
        "Job handle must not be inherited"
    );
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
        .env_clear()
        .envs(environment(root, mode))
        .current_dir(root);
    Ok(Created::Native(super::native::create(
        &command,
        false,
        job,
        stdio.map(|file| file.as_handle()),
    )?))
}

// The unassigned branch exists only to reproduce the old crash window deterministically.
fn create_suspended(
    job: Option<&Job>,
    root: &Path,
    mode: &str,
    stdio: [&File; 3],
) -> Result<Created> {
    let inherited = stdio.map(duplicate_inheritable);
    let inherited: Vec<_> = inherited.into_iter().collect::<Result<_>>()?;
    let handles: Vec<_> = inherited
        .iter()
        .map(|h| HANDLE(h.as_raw_handle()))
        .collect();
    let jobs = [job.map_or(HANDLE::default(), |job| job.0)];
    let attributes = Attributes::new()?;
    unsafe {
        if job.is_some() {
            UpdateProcThreadAttribute(
                attributes.list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                Some(jobs.as_ptr().cast()),
                std::mem::size_of_val(&jobs),
                None,
                None,
            )?;
        }
        UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(handles.as_ptr().cast()),
            handles.len() * std::mem::size_of::<HANDLE>(),
            None,
            None,
        )?;
    }
    let image = std::env::current_exe()?.canonicalize()?;
    let application = wide(image.as_os_str())?;
    let mut command = quoted(image.as_os_str())?;
    for arg in ["--exact", FIXTURE, "--ignored", "--nocapture"] {
        command.push(b' ' as u16);
        command.extend(quoted(OsStr::new(arg))?);
    }
    command.push(0);
    let mut block = Vec::new();
    for (key, value) in environment(root, mode) {
        let mut pair = key;
        pair.push("=");
        pair.push(value);
        block.extend(wide(&pair)?);
    }
    block.push(0);
    let cwd = wide(root.as_os_str())?;
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: handles[0],
            hStdOutput: handles[1],
            hStdError: handles[2],
            ..Default::default()
        },
        lpAttributeList: attributes.list,
    };
    let mut process = PROCESS_INFORMATION::default();
    // Both attributes stay alive through creation. Failure has no legacy spawn/attach fallback.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            PWSTR(command.as_mut_ptr()),
            None,
            None,
            true,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            Some(block.as_ptr().cast()),
            PCWSTR(cwd.as_ptr()),
            &startup.StartupInfo,
            &mut process,
        )?;
        Ok(Created::Legacy {
            process: OwnedHandle::from_raw_handle(process.hProcess.0),
            thread: OwnedHandle::from_raw_handle(process.hThread.0),
            pid: process.dwProcessId,
        })
    }
}

fn files(root: &Path) -> Result<[File; 3]> {
    fs::write(root.join("stdin.txt"), b"synthetic stdin")?;
    Ok([
        File::open(root.join("stdin.txt"))?,
        File::create(root.join("stdout.txt"))?,
        File::create(root.join("stderr.txt"))?,
    ])
}

#[test]
#[ignore = "Hidden synthetic parent/child process for Job-list tests"]
fn synthetic_process() -> Result<()> {
    let root = std::path::PathBuf::from(
        std::env::var_os("WX_JOB_LIST_SPIKE_ROOT").context("Missing synthetic root")?,
    );
    let mode = std::env::var("WX_JOB_LIST_SPIKE_MODE")?;
    if mode.starts_with("parent") {
        let job = if mode.ends_with("capture") {
            Job::for_account_capture()?
        } else {
            Job::new()?
        };
        let io = files(&root)?;
        let assigned = !mode.contains("legacy");
        let child_mode = if mode.contains("running") {
            "worker"
        } else {
            "child"
        };
        let child = if assigned {
            spawn_suspended(&job, &root, child_mode, [&io[0], &io[1], &io[2]])?
        } else {
            create_suspended(None, &root, "child", [&io[0], &io[1], &io[2]])?
        };
        let mut in_job = BOOL::default();
        unsafe {
            IsProcessInJob(child.handle(), job.0, &mut in_job)?;
        }
        ensure!(
            in_job.as_bool() == assigned,
            "Unexpected child Job membership"
        );
        fs::write(
            root.join("receipt.tmp"),
            format!("{} {}", child.id(), child.created()?),
        )?;
        fs::rename(root.join("receipt.tmp"), root.join("receipt"))?;
        if mode.contains("running") {
            child.resume()?;
        }
        // Barrier modes never resume; running modes also exercise capture descendants.
        let end = Instant::now() + Duration::from_secs(30);
        while root.is_dir() && Instant::now() < end {
            if root.join("stop").exists() {
                break;
            }
            if root.join("cancel").is_file() {
                job.start_terminate()?;
                fs::write(root.join("cancelled"), b"synthetic")?;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    } else if mode == "worker" {
        let mut app = Parent(
            Command::new(std::env::current_exe()?)
                .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
                .env("WX_JOB_LIST_SPIKE_MODE", "app")
                .creation_flags(CREATE_NO_WINDOW.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
            None,
        );
        let _ = app.0.wait()?;
    } else if mode == "app" {
        fs::write(
            root.join("app-receipt.tmp"),
            format!(
                "{} {}",
                std::process::id(),
                creation_time(unsafe { GetCurrentProcess() })?
            ),
        )?;
        fs::rename(root.join("app-receipt.tmp"), root.join("app-receipt"))?;
        let end = Instant::now() + Duration::from_secs(30);
        while root.is_dir() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    } else {
        ensure!(
            std::env::current_dir()?.canonicalize()? == root.canonicalize()?,
            "cwd changed"
        );
        ensure!(
            std::env::var("WX_JOB_LIST_SPIKE_VALUE")? == "synthetic value = preserved",
            "environment changed"
        );
        let mut stdin = String::new();
        std::io::stdin().read_to_string(&mut stdin)?;
        ensure!(stdin == "synthetic stdin", "stdin changed");
        std::io::stdout().write_all(b"synthetic stdout\n")?;
        std::io::stderr().write_all(b"synthetic stderr\n")?;
        fs::write(root.join("child-ran"), b"synthetic")?;
    }
    Ok(())
}

struct Parent(Child, Option<std::path::PathBuf>);
impl Drop for Parent {
    fn drop(&mut self) {
        if let Some(root) = &self.1 {
            // Before verified handle ownership, never kill the parent of an unassigned child.
            // A failed stop write still leaves the parent's bounded root-lifetime loop armed.
            let _ = fs::write(root.join("stop"), b"synthetic");
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end {
                if self.0.try_wait().is_ok_and(|status| status.is_some()) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            return;
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Observed(OwnedHandle);
impl Observed {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
    fn from_receipt(receipt: &str) -> Result<Self> {
        let (pid, created) = receipt.split_once(' ').context("Invalid receipt")?;
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                false,
                pid.parse::<u32>()?,
            )?
        };
        let owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        ensure!(
            creation_time(handle)? == created.parse::<u64>()?,
            "Process identity changed"
        );
        Ok(Self(owned))
    }
}
impl Drop for Observed {
    fn drop(&mut self) {
        // Keep cleanup armed even if an assertion fails while the old child is suspended.
        unsafe {
            let handle = HANDLE(self.0.as_raw_handle());
            let _ = TerminateProcess(handle, 1);
            let _ = WaitForSingleObject(handle, 5000);
        }
    }
}

#[test]
fn creating_in_job_closes_parent_crash_before_resume_window() -> Result<()> {
    for mode in [
        "parent",
        "parent-capture",
        "parent-legacy",
        "parent-legacy-capture",
    ] {
        crash_at_creation_barrier(mode)?;
    }
    Ok(())
}

#[test]
fn job_assignment_capture_app_survives_cancel_and_parent_crash() -> Result<()> {
    for capture in [false, true] {
        for crash in [false, true] {
            let root = tempfile::tempdir()?;
            let mode = if capture {
                "parent-running-capture"
            } else {
                "parent-running"
            };
            let mut parent = Parent(
                Command::new(std::env::current_exe()?)
                    .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
                    .env("WX_JOB_LIST_SPIKE_ROOT", root.path())
                    .env("WX_JOB_LIST_SPIKE_MODE", mode)
                    .creation_flags(CREATE_NO_WINDOW.0)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?,
                Some(root.path().to_path_buf()),
            );
            let end = Instant::now() + Duration::from_secs(10);
            let app = loop {
                if let Ok(receipt) = fs::read_to_string(root.path().join("app-receipt")) {
                    break Observed::from_receipt(&receipt)?;
                }
                ensure!(
                    parent.0.try_wait()?.is_none(),
                    "Synthetic parent exited early"
                );
                ensure!(Instant::now() < end, "Synthetic app receipt timeout");
                std::thread::sleep(Duration::from_millis(5));
            };
            let worker = Observed::from_receipt(&fs::read_to_string(root.path().join("receipt"))?)?;
            if crash {
                unsafe {
                    TerminateProcess(HANDLE(parent.0.as_raw_handle()), 73)?;
                }
                let _ = parent.0.wait()?;
            } else {
                fs::write(root.path().join("cancel"), b"synthetic")?;
            }
            ensure!(
                unsafe { WaitForSingleObject(worker.handle(), 5000) } == WAIT_OBJECT_0,
                "Worker survived: capture={capture}, crash={crash}"
            );
            let expected = if capture { WAIT_TIMEOUT } else { WAIT_OBJECT_0 };
            ensure!(
                unsafe { WaitForSingleObject(app.handle(), 5000) } == expected,
                "Unexpected app lifetime: capture={capture}, crash={crash}"
            );
        }
    }
    Ok(())
}

fn crash_at_creation_barrier(mode: &str) -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut parent = Parent(
        Command::new(std::env::current_exe()?)
            .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
            .env("WX_JOB_LIST_SPIKE_ROOT", root.path())
            .env("WX_JOB_LIST_SPIKE_MODE", mode)
            .creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
        Some(root.path().to_path_buf()),
    );
    let end = Instant::now() + Duration::from_secs(10);
    let receipt = loop {
        if let Ok(value) = fs::read_to_string(root.path().join("receipt")) {
            break value;
        }
        ensure!(
            parent.0.try_wait()?.is_none(),
            "Synthetic parent exited before receipt"
        );
        ensure!(Instant::now() < end, "Synthetic receipt timeout");
        std::thread::sleep(Duration::from_millis(5));
    };
    let owned = Observed::from_receipt(&receipt)?;
    let handle = owned.handle();
    ensure!(
        !root.path().join("child-ran").exists(),
        "Child unexpectedly resumed"
    );
    unsafe {
        TerminateProcess(HANDLE(parent.0.as_raw_handle()), 73)?;
    }
    let _ = parent.0.wait()?;
    let expected = if mode.contains("legacy") {
        WAIT_TIMEOUT
    } else {
        WAIT_OBJECT_0
    };
    ensure!(
        unsafe { WaitForSingleObject(handle, 5000) } == expected,
        "Unexpected creation-barrier outcome: {mode}"
    );
    ensure!(
        !root.path().join("child-ran").exists(),
        "Suspended child executed"
    );
    Ok(())
}

#[test]
fn job_assignment_cancel_reaps_suspended_normal_and_capture_workers() -> Result<()> {
    for capture in [false, true] {
        let root = tempfile::tempdir()?;
        let job = if capture {
            Job::for_account_capture()?
        } else {
            Job::new()?
        };
        let io = files(root.path())?;
        let child = spawn_suspended(&job, root.path(), "child", [&io[0], &io[1], &io[2]])?;
        job.start_terminate()?;
        ensure!(
            unsafe { WaitForSingleObject(child.handle(), 5000) } == WAIT_OBJECT_0,
            "Cancelled worker survived: capture={capture}"
        );
        ensure!(
            !root.path().join("child-ran").exists(),
            "Cancelled worker executed"
        );
    }
    Ok(())
}

#[test]
fn job_list_preserves_stdio_environment_and_cwd() -> Result<()> {
    let root = tempfile::tempdir()?;
    let job = Job::new()?;
    let io = files(root.path())?;
    let child = spawn_suspended(&job, root.path(), "child", [&io[0], &io[1], &io[2]])?;
    let mut in_job = BOOL::default();
    unsafe {
        IsProcessInJob(child.handle(), job.0, &mut in_job)?;
    }
    ensure!(in_job.as_bool(), "Child not owned before resume");
    child.resume()?;
    ensure!(
        unsafe { WaitForSingleObject(child.handle(), 5000) } == WAIT_OBJECT_0,
        "Synthetic child timeout"
    );
    let mut code = 1;
    unsafe {
        GetExitCodeProcess(child.handle(), &mut code)?;
    }
    ensure!(
        code == 0 && root.path().join("child-ran").is_file(),
        "Synthetic child failed"
    );
    ensure!(
        fs::read_to_string(root.path().join("stdout.txt"))?.contains("synthetic stdout"),
        "stdout changed"
    );
    ensure!(
        fs::read_to_string(root.path().join("stderr.txt"))?.contains("synthetic stderr"),
        "stderr changed"
    );
    Ok(())
}

#[test]
fn job_assignment_failure_never_falls_back_to_unowned_spawn() -> Result<()> {
    let root = tempfile::tempdir()?;
    let job = Job::new()?;
    let io = files(root.path())?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
    }
    let _first = spawn_suspended(&job, root.path(), "child", [&io[0], &io[1], &io[2]])?;
    ensure!(
        spawn_suspended(&job, root.path(), "child", [&io[0], &io[1], &io[2]]).is_err(),
        "Job limit unexpectedly bypassed"
    );
    ensure!(
        !root.path().join("child-ran").exists(),
        "Rejected worker ran"
    );
    Ok(())
}
