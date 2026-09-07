//! 两个本地 ASR 后端共用的 Job 与有界管道读取；不管理协议、模型或请求期限。
use anyhow::{bail, Result};
use std::io::Read;
#[cfg(windows)]
use {
    anyhow::{ensure, Context},
    std::{
        ffi::c_void,
        mem::size_of_val,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::Child,
        thread,
        time::{Duration, Instant},
    },
    windows::{
        core::PCWSTR,
        Win32::{Foundation::HANDLE, System::JobObjects::*},
    },
};

#[derive(Clone, Copy)]
pub(super) enum Caller {
    WhisperCpp,
    Python,
}

impl Caller {
    fn select(self, cpp: &'static str, python: &'static str) -> &'static str {
        match self {
            Self::WhisperCpp => cpp,
            Self::Python => python,
        }
    }
    #[cfg(windows)]
    fn job_error(self, action: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "{action} {} Job Object failed",
            self.select("ASR", "inference")
        )
    }
}

#[cfg(windows)]
pub(super) struct Job {
    // OwnedHandle 唯一关闭句柄，并允许随 Python 的 Mutex<Worker> 跨线程移动。
    handle: OwnedHandle,
    caller: Caller,
}

#[cfg(windows)]
impl Job {
    pub(super) fn attach(child: &Child, caller: Caller) -> Result<Self> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|_| caller.job_error("create"))?;
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle.0) },
            caller,
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // 绑定使用系统结构；失败时 OwnedHandle 仍会关闭 Job，保持原先的回收边界。
        unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
            .map_err(|_| caller.job_error("configure"))?;
            AssignProcessToJobObject(handle, HANDLE(child.as_raw_handle()))
                .map_err(|_| caller.job_error(caller.select("assign", "attach")))?;
        }
        Ok(job)
    }

    pub(super) fn terminate(&self) -> Result<()> {
        let handle = HANDLE(self.handle.as_raw_handle());
        unsafe { TerminateJobObject(handle, 1) }.map_err(|_| self.caller.job_error("terminate"))?;
        let start = Instant::now();
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            unsafe {
                QueryInformationJobObject(
                    handle,
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of_val(&accounting) as u32,
                    None,
                )
            }
            .map_err(|_| {
                self.caller
                    .job_error(self.caller.select("query", "inspect"))
            })?;
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            ensure!(
                start.elapsed() < Duration::from_secs(2),
                "{}",
                self.caller.select(
                    "ASR Job cleanup timed out",
                    "inference Job Object cleanup timed out"
                )
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// 每个管道每轮最多读 64 KiB；调用者按原顺序交替 stdout/stderr，共用累计限额。
#[cfg(windows)]
pub(super) fn drain<R: Read + AsRawHandle>(
    pipe: &mut R,
    total: &mut u64,
    limit: u64,
    caller: Caller,
) -> Result<()> {
    // 当前依赖未启用 Win32_System_Pipes，只保留这一份无结构体布局的原有 FFI。
    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(
            pipe: *mut c_void,
            buffer: *mut c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    let mut buffer = [0u8; 8192];
    for _ in 0..8 {
        let mut available = 0u32;
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
            if std::io::Error::last_os_error().raw_os_error() == Some(109) {
                return Ok(());
            }
            bail!("inspect {} pipe failed", caller.select("ASR", "inference"));
        }
        if available == 0 {
            break;
        }
        let read = pipe.read(&mut buffer[..available.min(8192) as usize]);
        let read = match caller {
            Caller::WhisperCpp => read.context("read ASR pipe"),
            Caller::Python => read.map_err(anyhow::Error::from),
        }?;
        if read == 0 {
            break;
        }
        *total = total.saturating_add(read as u64);
        ensure!(
            *total <= limit,
            "{}",
            caller.select(
                "ASR process stream limit exceeded",
                "local inference process output limit exceeded; output withheld"
            )
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub(super) fn drain<R: Read>(_: &mut R, _: &mut u64, _: u64, caller: Caller) -> Result<()> {
    bail!(
        "{}",
        caller.select(
            "ASR pipe supervision requires Windows",
            "local inference supervision requires Windows"
        )
    )
}
