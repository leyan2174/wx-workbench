//! Windows child ownership: suspended start, nested Job, bounded pipes and cleanup.
#[cfg(test)]
use anyhow::bail;
use anyhow::{ensure, Context, Result};
use std::{
    io::Read,
    os::windows::io::AsRawHandle,
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};
#[cfg(test)]
use windows::Win32::System::{
    Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    },
    Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::*,
    },
};

#[cfg(test)]
pub(crate) const SUSPENDED_NO_WINDOW: u32 = 0x0800_0004;
pub(crate) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[path = "managed/async_pipes.rs"]
mod async_pipes;
#[path = "managed/native.rs"]
mod native;
#[path = "managed/sync_pipes.rs"]
mod sync_pipes;
#[path = "managed/user_security.rs"]
mod user_security;
pub(crate) use user_security::with_user_security;

/// Only the daemon's two private worker entry points use this owner.
pub(crate) struct Child {
    process: native::Process,
    pub(crate) stdin: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    pub(crate) stdout: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    pub(crate) stderr: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

impl Child {
    pub(crate) fn id(&self) -> u32 {
        self.process.id()
    }
    pub(crate) fn handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        self.process.handle()
    }
    pub(crate) fn start_kill(&mut self) -> std::io::Result<()> {
        self.process.kill()
    }
    pub(crate) async fn kill(&mut self) -> std::io::Result<()> {
        self.start_kill()?;
        self.wait().await?;
        Ok(())
    }
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.process.try_wait()
    }
    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

pub(crate) async fn spawn_worker(
    command: &Command,
    inherit_environment: bool,
    job: &Job,
) -> Result<Child> {
    use std::os::windows::io::AsHandle;
    ensure!(
        std::path::Path::new(command.get_program()).is_absolute(),
        "Worker executable must be absolute"
    );
    let async_pipes::Prepared {
        stdin,
        stdout,
        stderr,
        child,
    } = async_pipes::prepare().await?;
    let process = native::create(
        command,
        inherit_environment,
        job,
        child.each_ref().map(|handle| handle.as_handle()),
    )?;
    // Only child-side copies in the new process remain; input shutdown/drop can deliver EOF.
    drop(child);
    process.resume()?;
    Ok(Child {
        process,
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

#[cfg(test)]
#[path = "managed/job_assignment.rs"]
mod job_assignment;

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

    #[cfg(test)]
    fn assign_and_resume(&self, handle: HANDLE, pid: u32) -> Result<()> {
        // No breakaway flag: an inner Job must remain owned by the worker's outer Job.
        // Incompatible host Jobs fail closed while the new child is still suspended.
        unsafe { AssignProcessToJobObject(self.0, handle)? };
        resume(pid)
    }

    #[cfg(test)]
    pub(crate) fn attach(&self, child: &tokio::process::Child) -> Result<()> {
        self.assign_and_resume(
            HANDLE(child.raw_handle().context("Missing child handle")?),
            child.id().context("Child exited before Job assignment")?,
        )
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

#[cfg(test)]
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
fn stop(child: &native::Process, job: &Job) -> Result<()> {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    let job_result = job.terminate_until(deadline);
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
    child: native::Process,
    job: Job,
    cleanup_attempted: bool,
}

impl Process {
    fn cleanup(&mut self) -> Result<()> {
        if self.cleanup_attempted {
            return Ok(());
        }
        self.cleanup_attempted = true;
        stop(&self.child, &self.job)
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
    inherit_environment: bool,
    deadline: Instant,
    limit: u64,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Output> {
    ensure!(limit > 0, "Child output limit must be positive");
    ensure!(!cancelled(), "Child process cancelled");
    ensure!(Instant::now() < deadline, "Child process deadline expired");
    use std::os::windows::io::AsHandle;
    let sync_pipes::Prepared {
        stdin,
        mut stdout,
        mut stderr,
        child_stdout,
        child_stderr,
    } = sync_pipes::prepare()?;
    let job = Job::new()?;
    let mut process = Process {
        child: native::create(
            command,
            inherit_environment,
            &job,
            [
                stdin.as_handle(),
                child_stdout.as_handle(),
                child_stderr.as_handle(),
            ],
        )
        .context("Start child process")?,
        job,
        cleanup_attempted: false,
    };
    drop((stdin, child_stdout, child_stderr));
    let result = (|| -> Result<Output> {
        process.child.resume()?;
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
