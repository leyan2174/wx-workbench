//! Process-exit races in the synthetic runtime cleanup, without any real account.
#![cfg(windows)]

#[path = "support/bootstrap.rs"]
#[allow(dead_code)]
mod bootstrap;

use std::{
    io::{self, Read},
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
};

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
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
}

const CHILD_ENV: &str = "WX_BOOTSTRAP_CLEANUP_TEST_CHILD";

struct SyntheticChild {
    child: Child,
    handle: OwnedHandle,
}

impl SyntheticChild {
    fn new() -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "cleanup_child_waits_for_eof", "--nocapture"])
            .env_clear()
            .env(CHILD_ENV, "1")
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        println!("COMMAND: {command:?}");
        let child = command.spawn().unwrap();
        // Same query/synchronize rights as the cleanup helper. Keep this exact
        // handle open across exit, so PID reuse cannot satisfy the assertion.
        let raw = unsafe { OpenProcess(0x00100000 | 0x1000, 0, child.id()) };
        assert!(!raw.is_null(), "open synthetic process failed");
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let result = Self { child, handle };
        result.assert_live();
        result
    }

    fn assert_live(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) },
            258
        );
    }

    fn exit(&mut self) {
        drop(self.child.stdin.take());
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 10_000) },
            0,
            "synthetic child failed to exit on stdin EOF"
        );
        assert!(self.child.wait().unwrap().success());
    }

    fn identity(&self) -> serde_json::Value {
        let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
        assert_ne!(
            unsafe {
                GetProcessTimes(
                    self.handle.as_raw_handle(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            },
            0
        );
        serde_json::json!({
            "pid": self.child.id(),
            "created": created,
            "exe": std::env::current_exe().unwrap(),
        })
    }
}

impl Drop for SyntheticChild {
    fn drop(&mut self) {
        // Cooperative EOF only: never kill a process, including on assertion failure.
        drop(self.child.stdin.take());
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 10_000) } == 0 {
            let _ = self.child.wait();
        }
    }
}

#[test]
fn cleanup_child_waits_for_eof() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let mut byte = [0];
    assert_eq!(io::stdin().read(&mut byte).unwrap(), 0);
}

#[test]
fn exited_original_handle_skips_image_query() {
    let mut child = SyntheticChild::new();
    child.exit();
    let result = bootstrap::process_image_if_running(&child.handle, || {
        panic!("an exited process must not need an image query")
    });
    assert!(result.unwrap().is_none());
}

#[test]
fn exit_during_failed_image_query_is_tolerated() {
    let mut child = SyntheticChild::new();
    let handle = child.handle.try_clone().unwrap();
    let result = bootstrap::process_image_if_running(&handle, || {
        child.assert_live();
        child.exit();
        // Deterministic failure at the OS query boundary, after a real process
        // exit. The helper must independently observe the original handle.
        Err(io::Error::from_raw_os_error(6))
    });
    assert!(result.unwrap().is_none());
}

#[test]
fn live_process_image_failure_is_not_swallowed() {
    let child = SyntheticChild::new();
    let error =
        bootstrap::process_image_if_running(&child.handle, || Err(io::Error::from_raw_os_error(5)))
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "read bootstrap process executable failed"
    );
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(5)
    );
    child.assert_live();
}

#[test]
fn live_process_image_is_still_returned_for_identity_validation() {
    let child = SyntheticChild::new();
    let actual = bootstrap::process_image_if_running(&child.handle, || {
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        if unsafe {
            QueryFullProcessImageNameW(
                child.handle.as_raw_handle(),
                0,
                name.as_mut_ptr(),
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(PathBuf::from(std::ffi::OsString::from_wide(
            &name[..length as usize],
        )))
    })
    .unwrap()
    .expect("live image query must not bypass executable validation");
    assert_eq!(
        actual.canonicalize().unwrap(),
        std::env::current_exe().unwrap().canonicalize().unwrap()
    );
    child.assert_live();
}

#[test]
fn live_process_birth_mismatch_is_rejected() {
    let child = SyntheticChild::new();
    let mut record = child.identity();
    record["created"] = serde_json::json!(record["created"].as_u64().unwrap() + 1);
    let error = bootstrap::verified_process(&record).unwrap_err();
    assert_eq!(error.to_string(), "bootstrap PID was reused");
    child.assert_live();
}

#[test]
fn live_process_executable_mismatch_is_rejected() {
    let child = SyntheticChild::new();
    let mut record = child.identity();
    let directory = tempfile::tempdir().unwrap();
    let wrong_executable = directory.path().join("not-the-child.exe");
    std::fs::write(
        &wrong_executable,
        b"synthetic identity mismatch, never executed",
    )
    .unwrap();
    record["exe"] = serde_json::json!(wrong_executable);
    let error = bootstrap::verified_process(&record).unwrap_err();
    assert_eq!(
        error.to_string(),
        "bootstrap executable is not this test binary"
    );
    child.assert_live();
}
