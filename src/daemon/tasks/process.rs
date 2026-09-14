//! Suspended start, Job assignment, then resume; descendants are owned by default.
pub use crate::windows_process::managed::Job;
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
        reap(child, job)
            .await
            .context("Unable to confirm failed worker cleanup")?;
        return Err(error);
    }
    Ok((child, job))
}

pub async fn reap(mut child: Child, job: Job) -> Result<()> {
    let deadline = tokio::time::Instant::now() + crate::windows_process::managed::CLEANUP_TIMEOUT;
    let termination = job.start_terminate();
    let _ = child.start_kill();
    tokio::time::timeout_at(deadline, child.wait())
        .await
        .context("Worker cleanup timed out; termination not confirmed")??;
    termination?;
    while !job.is_empty()? {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Worker descendant cleanup timed out; termination not confirmed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Ok(())
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
            reap(child, job).await?;
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
            reap(child, job).await?;
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
                reap(child, job).await?;
                return Err(error.into());
            }
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) };
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                reap(child, job).await?;
                return Err(error.into());
            }
        };
        let _owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        reap(child, job).await?;
        assert_eq!(unsafe { WaitForSingleObject(handle, 2000) }, WAIT_OBJECT_0);
        Ok(())
    }
}
