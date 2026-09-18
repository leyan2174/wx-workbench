//! Synchronous helper output; readers are polled by the existing bounded drain.
use anyhow::Result;
use std::{
    fs::File,
    os::windows::io::{FromRawHandle, OwnedHandle},
};

pub(super) struct Prepared {
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
    pub child_stdout: File,
    pub child_stderr: File,
}

fn pipe() -> Result<(File, File)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn CreatePipe(
            read: *mut *mut std::ffi::c_void,
            write: *mut *mut std::ffi::c_void,
            security: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    // NULL security attributes create non-inheritable handles; the native primitive
    // duplicates only child endpoints into its explicit HANDLE_LIST.
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe {
        (
            File::from(OwnedHandle::from_raw_handle(read)),
            File::from(OwnedHandle::from_raw_handle(write)),
        )
    })
}

pub(super) fn prepare() -> Result<Prepared> {
    let stdin = File::open("NUL")?;
    let (stdout, child_stdout) = pipe()?;
    let (stderr, child_stderr) = pipe()?;
    Ok(Prepared {
        stdin,
        stdout,
        stderr,
        child_stdout,
        child_stderr,
    })
}
