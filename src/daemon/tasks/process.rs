//! Suspended start, Job assignment, then resume; descendants are owned by default.
use crate::{
    runtime::RuntimeContext,
    service::{plan::Step, protocol::MAX_REQUEST_BYTES},
};
use anyhow::{ensure, Context, Result};
use std::process::Stdio;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
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
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    },
};

pub struct Job(HANDLE);
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
    pub(crate) fn attach(&self, child: &Child) -> Result<()> {
        let handle = HANDLE(child.raw_handle().context("Missing worker handle")?);
        unsafe {
            AssignProcessToJobObject(self.0, handle)?;
        }
        resume(child.id().context("Worker exited")?)
    }
    pub(crate) fn new() -> Result<Self> {
        Self::create(false)
    }

    pub(crate) fn for_account_capture() -> Result<Self> {
        Self::create(true)
    }

    fn create(account_capture: bool) -> Result<Self> {
        let job = Self(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if account_capture {
            // The capture worker stays owned; the user's restarted Weixin must outlive it.
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
        }
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            )?;
        }
        Ok(job)
    }
}

fn resume(pid: u32) -> Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)? };
    let result = (|| -> Result<()> {
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        unsafe {
            Thread32First(snapshot, &mut entry)?;
        }
        loop {
            if entry.th32OwnerProcessID == pid {
                let thread =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)? };
                let count = unsafe { ResumeThread(thread) };
                unsafe {
                    let _ = CloseHandle(thread);
                }
                ensure!(count != u32::MAX, "Unable to resume task worker");
                return Ok(());
            }
            if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
                break;
            }
        }
        anyhow::bail!("Suspended task thread not found")
    })();
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

pub async fn spawn(runtime: &RuntimeContext, step: &Step) -> Result<(Child, Job)> {
    let bytes = serde_json::to_vec(step)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_REQUEST_BYTES,
        "Worker frame exceeds limit"
    );
    let job = Job::new()?;
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .current_dir(
            runtime
                .config_path
                .parent()
                .context("Missing configuration parent")?,
        )
        .env("WX_CLI_CONFIG", &runtime.config_path)
        .env("WX_CLI_HOME", &runtime.root)
        .env("WX_CLI_EXPECTED_RUNTIME", &runtime.id)
        .env("WX_DAEMON_TASK_WORKER", "1")
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WECHAT_EXPORT_USERS")
        .env_remove("WECHAT_EXPORT_CONTACTS")
        .env_remove("WECHAT_EXPORT_FORMATS")
        .env_remove("WECHAT_EXPORT_IMAGES")
        .env_remove("WXWORK_EXPORT_CONVERSATIONS")
        .env("WECHAT_SNS_DOWNLOAD_MEDIA", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(0x08000004);
    let mut child = command.spawn().context("Unable to create task worker")?;
    let result = async {
        job.attach(&child)?;
        let mut input = child.stdin.take().context("Missing worker input")?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            input.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
            input.write_all(&bytes).await?;
            input.shutdown().await
        })
        .await??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        reap(child, job).await;
        return Err(error);
    }
    Ok((child, job))
}

pub async fn reap(mut child: Child, job: Job) {
    drop(job);
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = "daemon::tasks::process::tests::process_tree_fixture";

    #[test]
    #[ignore = "由 Windows Job 生命周期测试启动的隐藏子进程样本"]
    #[expect(
        clippy::zombie_processes,
        reason = "故意模拟父进程先退出；后代由外层 Windows Job 测试回收"
    )]
    fn process_tree_fixture() {
        use std::os::windows::process::CommandExt;
        let root = std::path::PathBuf::from(
            std::env::var_os("WX_TEST_JOB_DIRECTORY").expect("test directory"),
        );
        if std::env::var("WX_TEST_JOB_DESCENDANT").is_err() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
                .env("WX_TEST_JOB_DESCENDANT", "1")
                .creation_flags(0x08000000)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            if std::env::var_os("WX_TEST_JOB_PARENT_EXIT").is_none() {
                let _ = child.wait();
            }
        } else {
            std::fs::write(root.join("descendant.pid"), std::process::id().to_string()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            // 握手前失败时，临时目录回收也会让样本退出，避免遗留进程。
            while root.is_dir() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    #[tokio::test]
    async fn account_capture_preserves_app_after_completion_and_cancellation() -> Result<()> {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{
                OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE,
            },
        };
        for completed in [false, true] {
            let root = tempfile::tempdir()?;
            let job = Job::for_account_capture()?;
            let mut command = Command::new(std::env::current_exe()?);
            command
                .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
                .env("WX_TEST_JOB_DIRECTORY", root.path())
                .env_remove("WX_TEST_JOB_DESCENDANT")
                .env_remove("WX_TEST_JOB_PARENT_EXIT")
                .creation_flags(0x08000004)
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if completed {
                command.env("WX_TEST_JOB_PARENT_EXIT", "1");
            }
            let mut child = command.spawn()?;
            job.attach(&child)?;
            let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Ok(value) = std::fs::read_to_string(root.path().join("descendant.pid")) {
                        if let Ok(pid) = value.parse::<u32>() {
                            break pid;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await?;
            let handle =
                unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid)? };
            let _owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
            let worker_completed = if completed {
                tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
                    .await
                    .is_ok_and(|status| status.is_ok_and(|status| status.success()))
            } else {
                child.try_wait()?.is_none()
            };
            reap(child, job).await;
            let app_survived = unsafe { WaitForSingleObject(handle, 200) } == WAIT_TIMEOUT;
            // Only terminate the fixture we hold a handle to, even if an assertion fails.
            unsafe {
                TerminateProcess(handle, 0)?;
            }
            assert_eq!(unsafe { WaitForSingleObject(handle, 2000) }, WAIT_OBJECT_0);
            assert!(
                worker_completed,
                "unexpected worker lifecycle: completed={completed}"
            );
            assert!(app_survived, "app was reaped: completed={completed}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn suspended_worker_is_attached_before_running_and_reap_kills_descendants() -> Result<()>
    {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        let root = tempfile::tempdir()?;
        let marker = root.path().join("descendant.pid");
        let job = Job::new()?;
        let child = Command::new(std::env::current_exe()?)
            .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
            .env("WX_TEST_JOB_DIRECTORY", root.path())
            .env_remove("WX_TEST_JOB_DESCENDANT")
            .creation_flags(0x08000004)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        assert!(!marker.exists());
        if let Err(error) = job.attach(&child) {
            reap(child, job).await;
            return Err(error);
        }
        let descendant = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(value) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = value.parse::<u32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        let pid = match descendant {
            Ok(pid) => pid,
            Err(error) => {
                reap(child, job).await;
                return Err(error.into());
            }
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) };
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                reap(child, job).await;
                return Err(error.into());
            }
        };
        let _owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        reap(child, job).await;
        assert_eq!(unsafe { WaitForSingleObject(handle, 2000) }, WAIT_OBJECT_0);
        Ok(())
    }
}
