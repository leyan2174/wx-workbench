use super::*;
use std::io::Write;
use windows::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

fn helper(mode: &str, root: &std::path::Path) -> Command {
    // libtest names omit the crate name but retain any fixture-host module prefix.
    let module = module_path!().split_once("::").unwrap().1;
    let fixture = format!("{module}::helper_process");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", fixture.as_str(), "--ignored", "--nocapture"])
        .env("WX_LIFECYCLE_MODE", mode)
        .env("WX_LIFECYCLE_ROOT", root);
    command
}

#[test]
#[ignore = "Synthetic process fixture, launched only by lifecycle tests"]
fn helper_process() {
    let mode = std::env::var("WX_LIFECYCLE_MODE").unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("WX_LIFECYCLE_ROOT").unwrap());
    if mode == "success" {
        println!("synthetic stdout");
        eprintln!("synthetic stderr");
        return;
    }
    if mode == "nested" || mode == "nested-hang" {
        let mut command = helper("tree", &root);
        let seconds = if mode == "nested" { 2 } else { 20 };
        let result = output(
            &mut command,
            Instant::now() + Duration::from_secs(seconds),
            65536,
            || false,
        );
        assert!(result.unwrap_err().to_string().contains("deadline"));
        std::fs::write(root.join("nested-complete"), b"ok").unwrap();
        return;
    }
    std::fs::write(
        root.join(format!("{mode}.pid")),
        std::process::id().to_string(),
    )
    .unwrap();
    if mode == "flood" {
        let block = [b'x'; 8192];
        loop {
            std::io::stdout().write_all(&block).unwrap();
            std::io::stderr().write_all(&block).unwrap();
        }
    }
    if mode == "tree" || mode == "orphan" {
        let mut command = helper("descendant", &root);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x0800_0000);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("descendant.pid").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if mode == "orphan" {
            return;
        }
        let _ = child.wait();
        return;
    }
    // Bounded safety net if an assertion in the supervising process fails.
    let deadline = Instant::now() + Duration::from_secs(30);
    while root.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_dead(root: &std::path::Path, mode: &str) {
    let pid: u32 = std::fs::read_to_string(root.join(format!("{mode}.pid")))
        .unwrap()
        .parse()
        .unwrap();
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
        // An already-reaped PID no longer has a process object.
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(87));
        return;
    };
    // Job accounting can reach zero shortly before the process object is signaled.
    let wait = unsafe { WaitForSingleObject(handle, 2000) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    assert_ne!(wait, WAIT_TIMEOUT, "synthetic {mode} still alive");
    assert_eq!(wait, WAIT_OBJECT_0);
}

#[test]
fn captures_both_pipes() {
    let root = tempfile::tempdir().unwrap();
    let result = output(
        &mut helper("success", root.path()),
        Instant::now() + Duration::from_secs(5),
        65536,
        || false,
    )
    .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("synthetic stdout"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("synthetic stderr"));
}

#[test]
fn deadline_terminates_hanging_process_and_descendants() {
    for mode in ["hang", "tree"] {
        let root = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let error = output(
            &mut helper(mode, root.path()),
            start + Duration::from_secs(2),
            65536,
            || false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"), "{error:#}");
        assert!(start.elapsed() < Duration::from_secs(6));
        assert_dead(root.path(), mode);
        if mode == "tree" {
            assert_dead(root.path(), "descendant");
        }
    }
}

#[test]
fn output_flood_is_bounded_and_reaped() {
    let root = tempfile::tempdir().unwrap();
    let error = output(
        &mut helper("flood", root.path()),
        Instant::now() + Duration::from_secs(5),
        16384,
        || false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("output limit"), "{error:#}");
    assert_dead(root.path(), "flood");
}

#[test]
fn cancellation_terminates_actual_tree() {
    let root = tempfile::tempdir().unwrap();
    let error = output(
        &mut helper("tree", root.path()),
        Instant::now() + Duration::from_secs(5),
        65536,
        || root.path().join("descendant.pid").exists(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert_dead(root.path(), "tree");
    assert_dead(root.path(), "descendant");
}

#[test]
fn successful_parent_exit_also_reaps_descendant() {
    let root = tempfile::tempdir().unwrap();
    let result = output(
        &mut helper("orphan", root.path()),
        Instant::now() + Duration::from_secs(5),
        65536,
        || false,
    )
    .unwrap();
    assert!(result.status.success());
    assert_dead(root.path(), "descendant");
}

#[test]
fn inner_job_cleanup_preserves_outer_worker() {
    let root = tempfile::tempdir().unwrap();
    let result = output(
        &mut helper("nested", root.path()),
        Instant::now() + Duration::from_secs(8),
        65536,
        || false,
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(root.path().join("nested-complete").is_file());
    assert_dead(root.path(), "descendant");
}

#[test]
fn expired_deadline_does_not_spawn() {
    let root = tempfile::tempdir().unwrap();
    assert!(output(
        &mut helper("hang", root.path()),
        Instant::now(),
        1024,
        || false
    )
    .is_err());
    assert!(!root.path().join("hang.pid").exists());
}

#[test]
fn outer_job_termination_also_terminates_inner_job_tree() {
    let root = tempfile::tempdir().unwrap();
    let error = output(
        &mut helper("nested-hang", root.path()),
        Instant::now() + Duration::from_secs(5),
        65536,
        || root.path().join("descendant.pid").exists(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert_dead(root.path(), "tree");
    assert_dead(root.path(), "descendant");
    assert!(!root.path().join("nested-complete").exists());
}
