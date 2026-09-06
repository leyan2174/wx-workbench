//! Keep long-lived children from retaining the CLI caller's output pipes.
use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

pub fn isolate_standard_handles() -> windows::core::Result<()> {
    // Run once, before threads or child processes exist. Explicit Stdio::inherit
    // still works: Rust duplicates the selected handle for that child.
    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(kind)? };
        if !handle.is_invalid() {
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))? };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };

    #[test]
    #[ignore = "Subprocess fixture for pipe EOF regression tests"]
    fn background_parent_fixture() {
        use std::os::windows::process::CommandExt;
        let Ok(path) = std::env::var("WX_PIPE_TEST_PID_FILE") else {
            return;
        };
        if std::env::var_os("WX_PIPE_TEST_SKIP_FIX").is_none() {
            isolate_standard_handles().unwrap();
        }
        let args = [
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            "Start-Sleep -Seconds 30",
        ];
        let pid = if std::env::var_os("WX_PIPE_TEST_FRIDA").is_some() {
            eprintln!("fixture: obtaining Frida");
            let frida = unsafe { frida::Frida::obtain() };
            let manager = frida::DeviceManager::obtain(&frida);
            let mut device = manager.get_local_device().unwrap();
            let options = frida::SpawnOptions::new().argv(args);
            eprintln!("fixture: spawning sleeper");
            let pid = device
                .spawn(
                    "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
                    &options,
                )
                .unwrap();
            std::fs::write(&path, pid.to_string()).unwrap();
            eprintln!("fixture: resuming sleeper");
            device.resume(pid).unwrap();
            eprintln!("fixture: dropping Frida");
            pid
        } else {
            Command::new(args[0])
                .args(&args[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap()
                .id()
        };
        std::fs::write(path, pid.to_string()).unwrap();
        println!("parent finished");
    }

    fn assert_pipe_eof(frida: bool) {
        let file = std::env::temp_dir().join(format!("wx-pipe-{}-{frida}.pid", std::process::id()));
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "windows_process::tests::background_parent_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("WX_PIPE_TEST_PID_FILE", &file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        if frida {
            cmd.env("WX_PIPE_TEST_FRIDA", "1");
        }
        let mut parent = cmd.spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        for mut pipe in [
            Box::new(parent.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(parent.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut data = String::new();
                let result = pipe.read_to_string(&mut data);
                let _ = tx.send((result, data));
            });
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = parent.try_wait().unwrap() {
                break Some(status);
            }
            if std::time::Instant::now() >= deadline {
                let _ = parent.kill();
                let _ = parent.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let first = rx.recv_timeout(Duration::from_secs(2));
        let second = rx.recv_timeout(Duration::from_secs(2));
        // Clean up our own sleeper even when the EOF assertion fails.
        let pid: u32 = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| {
                panic!("missing child PID: {error}; stdout={first:?}; stderr={second:?}")
            })
            .parse()
            .unwrap();
        let mut code = 0;
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                false,
                pid,
            )
            .unwrap();
            GetExitCodeProcess(handle, &mut code).unwrap();
            let _ = TerminateProcess(handle, 0);
            let _ = CloseHandle(handle);
        }
        let _ = std::fs::remove_file(file);
        assert!(
            status.is_some_and(|s| s.success()),
            "parent did not exit successfully: stdout={first:?}; stderr={second:?}"
        );
        assert_eq!(code, 259, "background child should still be alive at EOF");
        assert!(
            first.is_ok() && second.is_ok(),
            "background child retained caller output pipes"
        );
        for (result, _) in [first.unwrap(), second.unwrap()] {
            result.unwrap();
        }
    }

    #[test]
    fn background_child_does_not_hold_output_pipes() {
        assert_pipe_eof(false);
    }

    #[test]
    fn frida_child_does_not_hold_output_pipes() {
        assert_pipe_eof(true);
    }
}
