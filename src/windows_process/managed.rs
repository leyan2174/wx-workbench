//! Windows child ownership: suspended start, nested Job, bounded pipes and cleanup.
use anyhow::{bail, ensure, Context, Result};
use std::{
    io::Read,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::*,
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    },
};

pub(crate) const SUSPENDED_NO_WINDOW: u32 = 0x0800_0004;
pub(crate) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Job(HANDLE);
// The handle owns a kernel Job, with no thread-affine state.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
            let _ = CloseHandle(self.0);
        }
    }
}

impl Job {
    pub(crate) fn new() -> Result<Self> {
        Self::create(false)
    }

    pub(crate) fn for_account_capture() -> Result<Self> {
        Self::create(true)
    }

    fn create(account_capture: bool) -> Result<Self> {
        let job = Self(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if account_capture {
            // Only account capture permits the explicitly restarted user app to escape.
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        }
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )?;
        }
        Ok(job)
    }

    fn assign_and_resume(&self, handle: HANDLE, pid: u32) -> Result<()> {
        // No breakaway flag: an inner Job must remain owned by the worker's outer Job.
        // Incompatible host Jobs fail closed while the new child is still suspended.
        unsafe { AssignProcessToJobObject(self.0, handle)? };
        resume(pid)
    }

    pub(crate) fn attach(&self, child: &tokio::process::Child) -> Result<()> {
        self.assign_and_resume(
            HANDLE(child.raw_handle().context("Missing child handle")?),
            child.id().context("Child exited before Job assignment")?,
        )
    }

    pub(crate) fn attach_suspended(child: &Child) -> Result<Self> {
        let job = Self::new()?;
        job.assign_and_resume(HANDLE(child.as_raw_handle()), child.id())?;
        Ok(job)
    }

    pub(crate) fn terminate_until(&self, deadline: Instant) -> Result<()> {
        self.start_terminate()?;
        loop {
            if self.is_empty()? {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Child Job cleanup timed out; termination not confirmed"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn start_terminate(&self) -> Result<()> {
        unsafe { TerminateJobObject(self.0, 1)? };
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> Result<bool> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
                None,
            )?;
        }
        Ok(accounting.ActiveProcesses == 0)
    }
}

fn resume(pid: u32) -> Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)? };
    let result = (|| -> Result<()> {
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        unsafe { Thread32First(snapshot, &mut entry)? };
        loop {
            if entry.th32OwnerProcessID == pid {
                let handle =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)? };
                let count = unsafe { ResumeThread(handle) };
                unsafe {
                    let _ = CloseHandle(handle);
                }
                ensure!(count != u32::MAX, "Unable to resume child process");
                return Ok(());
            }
            if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
                bail!("Suspended child thread not found");
            }
        }
    })();
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

/// Poll only bytes already available; alternate pipes at most every 64 KiB.
/// No reader threads can be stranded by a descendant retaining an output handle.
pub(crate) fn drain<R: Read + AsRawHandle>(
    pipe: &mut R,
    total: &mut u64,
    limit: u64,
    mut capture: Option<&mut Vec<u8>>,
) -> Result<usize> {
    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(
            pipe: *mut std::ffi::c_void,
            buffer: *mut std::ffi::c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    let mut buffer = [0u8; 8192];
    let mut drained = 0;
    for _ in 0..8 {
        let mut available = 0;
        let ok = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(109) {
                break;
            }
            return Err(error).context("Inspect child output pipe");
        }
        if available == 0 {
            break;
        }
        let read = pipe.read(&mut buffer[..available.min(8192) as usize])?;
        if read == 0 {
            break;
        }
        *total = total.saturating_add(read as u64);
        ensure!(
            *total <= limit,
            "Child process output limit exceeded; output withheld"
        );
        if let Some(output) = capture.as_deref_mut() {
            output.extend_from_slice(&buffer[..read]);
        }
        drained += read;
    }
    Ok(drained)
}

/// Cleanup has its own small, finite budget, even after the operation deadline expires.
pub(crate) fn stop(child: &mut Child, job: Option<&Job>) -> Result<()> {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    let job_result = job.map_or(Ok(()), |job| job.terminate_until(deadline));
    // Also handles Job assignment failure (the direct child is still suspended).
    let _ = child.kill();
    loop {
        if child.try_wait().context("Reap child process")?.is_some() {
            return job_result;
        }
        ensure!(
            Instant::now() < deadline,
            "Child cleanup timed out; termination not confirmed"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

struct Process {
    child: Child,
    job: Option<Job>,
    cleanup_attempted: bool,
}

impl Process {
    fn cleanup(&mut self) -> Result<()> {
        if self.cleanup_attempted {
            return Ok(());
        }
        self.cleanup_attempted = true;
        stop(&mut self.child, self.job.as_ref())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Execute a one-shot helper. `deadline` covers spawn, both pipes and process exit.
/// Cancellation is checked between bounded drain rounds; cleanup is never just detaching.
pub(crate) fn output(
    command: &mut Command,
    deadline: Instant,
    limit: u64,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Output> {
    ensure!(limit > 0, "Child output limit must be positive");
    ensure!(!cancelled(), "Child process cancelled");
    ensure!(Instant::now() < deadline, "Child process deadline expired");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(SUSPENDED_NO_WINDOW);
    let mut process = Process {
        child: command.spawn().context("Start child process")?,
        job: None,
        cleanup_attempted: false,
    };
    let result = (|| -> Result<Output> {
        process.job = Some(Job::attach_suspended(&process.child)?);
        let mut stdout = process
            .child
            .stdout
            .take()
            .context("Missing child stdout")?;
        let mut stderr = process
            .child
            .stderr
            .take()
            .context("Missing child stderr")?;
        let (mut out, mut err, mut total) = (Vec::new(), Vec::new(), 0);
        let status = loop {
            ensure!(!cancelled(), "Child process cancelled");
            ensure!(Instant::now() < deadline, "Child process deadline expired");
            drain(&mut stdout, &mut total, limit, Some(&mut out))?;
            drain(&mut stderr, &mut total, limit, Some(&mut err))?;
            if let Some(status) = process.child.try_wait()? {
                break status;
            }
            thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            );
        };
        process.cleanup()?;
        loop {
            ensure!(!cancelled(), "Child process cancelled");
            ensure!(Instant::now() < deadline, "Child process deadline expired");
            let count = drain(&mut stdout, &mut total, limit, Some(&mut out))?
                + drain(&mut stderr, &mut total, limit, Some(&mut err))?;
            if count == 0 {
                break;
            }
        }
        Ok(Output {
            status,
            stdout: out,
            stderr: err,
        })
    })();
    let cleanup = process.cleanup();
    // A cleanup failure is not reported as successful cancellation or completion.
    cleanup.context("Unable to confirm child process cleanup")?;
    result
}

#[cfg(test)]
#[path = "managed/tests.rs"]
mod tests;
