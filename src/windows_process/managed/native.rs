//! Private creation primitive for the existing managed worker/helper call sites.
use super::Job;
use anyhow::{ensure, Context, Result};
use std::{
    cmp::Ordering,
    ffi::{OsStr, OsString},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::{Command, ExitStatus},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            DuplicateHandle, GetHandleInformation, DUPLICATE_SAME_ACCESS, HANDLE,
            HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::Threading::*,
    },
};

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

pub(super) struct Process {
    handle: OwnedHandle,
    thread: OwnedHandle,
    pid: u32,
}
impl Process {
    pub(super) fn id(&self) -> u32 {
        self.pid
    }
    pub(super) fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
    pub(super) fn resume(&self) -> Result<()> {
        ensure!(
            unsafe { ResumeThread(HANDLE(self.thread.as_raw_handle())) } != u32::MAX,
            "Resume managed child failed"
        );
        Ok(())
    }
    pub(super) fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        let handle = HANDLE(self.handle.as_raw_handle());
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                unsafe { GetExitCodeProcess(handle, &mut code) }.map_err(io::Error::other)?;
                Ok(Some(ExitStatus::from_raw(code)))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        if self.try_wait()?.is_none() {
            unsafe { TerminateProcess(HANDLE(self.handle.as_raw_handle()), 1) }
                .map_err(io::Error::other)?;
        }
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut units: Vec<_> = value.encode_wide().collect();
    ensure!(!units.contains(&0), "Managed child string contains NUL");
    units.push(0);
    Ok(units)
}
fn quoted(value: &OsStr) -> Result<Vec<u16>> {
    let raw = wide(value)?;
    let mut result = vec![u16::from(b'"')];
    let mut slashes = 0;
    for &unit in &raw[..raw.len() - 1] {
        if unit == u16::from(b'\\') {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            u16::from(b'\\'),
            if unit == u16::from(b'"') {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    result.push(u16::from(b'"'));
    Ok(result)
}

fn compare_keys(left: &OsStr, right: &OsStr) -> Result<Ordering> {
    #[link(name = "kernel32")]
    extern "system" {
        fn CompareStringOrdinal(
            left: *const u16,
            left_len: i32,
            right: *const u16,
            right_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len().try_into()?,
            right.as_ptr(),
            right.len().try_into()?,
            1,
        )
    };
    ensure!(
        (1..=3).contains(&result),
        "Environment name comparison failed"
    );
    Ok(result.cmp(&2))
}

// The caller supplies the inheritance policy explicitly; Command has no public env_clear getter.
pub(super) fn environment(command: &Command, inherit: bool) -> Result<Vec<u16>> {
    let mut values: Vec<(OsString, OsString)> = if inherit {
        std::env::vars_os().collect()
    } else {
        Vec::new()
    };
    for (key, value) in command.get_envs() {
        let mut index = 0;
        while index < values.len() {
            if compare_keys(&values[index].0, key)? == Ordering::Equal {
                values.remove(index);
            } else {
                index += 1;
            }
        }
        if let Some(value) = value {
            values.push((key.to_owned(), value.to_owned()));
        }
    }
    let mut failure = false;
    values.sort_by(|left, right| match compare_keys(&left.0, &right.0) {
        Ok(order) => order,
        Err(_) => {
            failure = true;
            Ordering::Equal
        }
    });
    ensure!(!failure, "Environment ordering failed");
    let mut block = Vec::new();
    for (key, value) in values {
        let key_units = wide(&key)?;
        ensure!(
            key_units.len() > 1 && !key_units[1..].contains(&u16::from(b'=')),
            "Invalid environment name"
        );
        let mut pair = key;
        pair.push("=");
        pair.push(value);
        block.extend(wide(&pair)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn duplicate_inheritable(source: BorrowedHandle<'_>) -> Result<OwnedHandle> {
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

pub(super) fn create(
    command: &Command,
    inherit_env: bool,
    job: &Job,
    stdio: [BorrowedHandle<'_>; 3],
) -> Result<Process> {
    create_in_jobs(command, inherit_env, &[job.0], stdio)
}

fn create_in_jobs(
    command: &Command,
    inherit_env: bool,
    jobs: &[HANDLE],
    stdio: [BorrowedHandle<'_>; 3],
) -> Result<Process> {
    ensure!(!jobs.is_empty(), "Managed child requires Job ownership");
    for job in jobs {
        let mut flags = 0;
        unsafe {
            GetHandleInformation(*job, &mut flags)?;
        }
        ensure!(
            flags & HANDLE_FLAG_INHERIT.0 == 0,
            "Managed Job handle is inheritable"
        );
    }
    // The audited call sites supply real executable paths, never shell/batch commands.
    let image = std::path::Path::new(command.get_program());
    ensure!(
        image
            .extension()
            .is_none_or(|value| value.eq_ignore_ascii_case("exe")),
        "Managed executable must be an exe"
    );
    let application = wide(image.as_os_str())?;
    let mut arguments = quoted(image.as_os_str())?;
    for arg in command.get_args() {
        arguments.push(u16::from(b' '));
        arguments.extend(quoted(arg)?);
    }
    arguments.push(0);
    ensure!(
        arguments.len() <= 32767,
        "Managed command line exceeds Windows limit"
    );
    let environment = environment(command, inherit_env)?;
    let cwd = command
        .get_current_dir()
        .map(|path| wide(path.as_os_str()))
        .transpose()?;
    let duplicates = stdio
        .map(duplicate_inheritable)
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let handles: Vec<_> = duplicates
        .iter()
        .map(|handle| HANDLE(handle.as_raw_handle()))
        .collect();
    let attributes = Attributes::new()?;
    unsafe {
        UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            Some(jobs.as_ptr().cast()),
            std::mem::size_of_val(jobs),
            None,
            None,
        )
        .context("Configure creation-time Job ownership")?;
        UpdateProcThreadAttribute(
            attributes.list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(handles.as_ptr().cast()),
            std::mem::size_of_val(handles.as_slice()),
            None,
            None,
        )
        .context("Configure child stdio handle list")?;
    }
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
    let mut information = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            if image.is_absolute() {
                PCWSTR(application.as_ptr())
            } else {
                PCWSTR::null()
            },
            PWSTR(arguments.as_mut_ptr()),
            None,
            None,
            true,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            Some(environment.as_ptr().cast()),
            cwd.as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            &startup.StartupInfo,
            &mut information,
        )
        .context("Create managed child in Job")?;
        Ok(Process {
            handle: OwnedHandle::from_raw_handle(information.hProcess.0),
            thread: OwnedHandle::from_raw_handle(information.hThread.0),
            pid: information.dwProcessId,
        })
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
