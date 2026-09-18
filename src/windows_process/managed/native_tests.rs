use super::*;
use std::{
    fs::{self, File},
    os::windows::io::AsHandle,
};
use windows::Win32::{Foundation::BOOL, System::JobObjects::*};

fn command(root: &std::path::Path) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .args([
            "--exact",
            "windows_process::managed::job_assignment::synthetic_process",
            "--ignored",
            "--nocapture",
        ])
        .env("WX_JOB_LIST_SPIKE_ROOT", root)
        .env("WX_JOB_LIST_SPIKE_MODE", "child")
        .env("WX_JOB_LIST_SPIKE_VALUE", "synthetic value = preserved")
        .current_dir(root);
    Ok(command)
}
fn stdio(root: &std::path::Path) -> Result<[File; 3]> {
    fs::write(root.join("stdin"), b"synthetic stdin")?;
    Ok([
        File::open(root.join("stdin"))?,
        File::create(root.join("stdout"))?,
        File::create(root.join("stderr"))?,
    ])
}

#[test]
fn compatible_nested_jobs_own_child_before_resume() -> Result<()> {
    let root = tempfile::tempdir()?;
    let outer = Job::new()?;
    let inner = Job::new()?;
    let stdio = stdio(root.path())?;
    let child = create_in_jobs(
        &command(root.path())?,
        true,
        &[outer.0, inner.0],
        stdio.each_ref().map(|file| file.as_handle()),
    )?;
    for job in [&outer, &inner] {
        let mut owned = BOOL::default();
        unsafe {
            IsProcessInJob(HANDLE(child.handle().as_raw_handle()), job.0, &mut owned)?;
        }
        ensure!(
            owned.as_bool(),
            "Nested Job missing ownership before resume"
        );
    }
    ensure!(
        !root.path().join("child-ran").exists(),
        "Suspended child ran"
    );
    outer.start_terminate()?;
    ensure!(
        unsafe { WaitForSingleObject(HANDLE(child.handle().as_raw_handle()), 5000) }
            == WAIT_OBJECT_0,
        "Outer Job did not reap child"
    );
    Ok(())
}

#[test]
fn incompatible_nonempty_job_hierarchies_fail_closed() -> Result<()> {
    let root = tempfile::tempdir()?;
    let outer = Job::new()?;
    let inner = Job::new()?;
    let stdio = stdio(root.path())?;
    let command = command(root.path())?;
    // Two already-populated disjoint Jobs cannot become a valid nested hierarchy.
    let left = create(
        &command,
        true,
        &outer,
        stdio.each_ref().map(|file| file.as_handle()),
    )?;
    let _right = create(
        &command,
        true,
        &inner,
        stdio.each_ref().map(|file| file.as_handle()),
    )?;
    ensure!(
        unsafe { AssignProcessToJobObject(inner.0, HANDLE(left.handle().as_raw_handle())) }
            .is_err(),
        "Control hierarchy is not incompatible"
    );
    let result = create_in_jobs(
        &command,
        true,
        &[outer.0, inner.0],
        stdio.each_ref().map(|file| file.as_handle()),
    );
    ensure!(result.is_err(), "Incompatible host Job was bypassed");
    ensure!(
        !root.path().join("child-ran").exists(),
        "Rejected child ran"
    );
    Ok(())
}

#[test]
#[ignore = "Synthetic process inside a host Job with an active-process limit"]
fn inherited_host_fixture() -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let root = std::path::PathBuf::from(
        std::env::var_os("WX_NATIVE_HOST_ROOT").context("Missing host fixture root")?,
    );
    let inner = Job::new()?;
    let io = stdio(&root)?;
    let mut command = command(&root)?;
    let old = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0)
        .spawn();
    if let Ok(mut child) = old {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("Host limit did not reject the legacy control");
    }
    ensure!(
        create(
            &command,
            true,
            &inner,
            io.each_ref().map(|file| file.as_handle())
        )
        .is_err(),
        "Creation-time Job escaped inherited host limit"
    );
    fs::write(root.join("host-refused"), b"synthetic")?;
    Ok(())
}

#[test]
fn inherited_host_limits_are_not_bypassed_by_job_list() -> Result<()> {
    let root = tempfile::tempdir()?;
    let outer = Job::new()?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    unsafe {
        SetInformationJobObject(
            outer.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
    }
    let io = stdio(root.path())?;
    let fixture = format!(
        "{}::inherited_host_fixture",
        module_path!().split_once("::").context("Fixture module")?.1
    );
    let mut command = Command::new(std::env::current_exe()?.canonicalize()?);
    command
        .args(["--exact", &fixture, "--ignored", "--nocapture"])
        .env("WX_NATIVE_HOST_ROOT", root.path());
    let parent = create(
        &command,
        true,
        &outer,
        io.each_ref().map(|file| file.as_handle()),
    )?;
    parent.resume()?;
    ensure!(
        unsafe { WaitForSingleObject(HANDLE(parent.handle().as_raw_handle()), 5000) }
            == WAIT_OBJECT_0,
        "Host fixture timed out"
    );
    ensure!(
        parent.try_wait()?.is_some_and(|status| status.success())
            && root.path().join("host-refused").is_file(),
        "Inherited host restriction was not preserved"
    );
    Ok(())
}

#[test]
fn environment_clear_and_case_insensitive_removal_preserve_utf16() -> Result<()> {
    use std::os::windows::ffi::OsStringExt;
    let mut command = Command::new("C:\\synthetic.exe");
    let value = OsString::from_wide(&[0xD800, b'x' as u16]);
    command.env("WX_NATIVE_SYNTHETIC", &value).env("empty", "");
    let expected: Vec<u16> = "empty=\0WX_NATIVE_SYNTHETIC="
        .encode_utf16()
        .chain(value.encode_wide())
        .chain([0, 0])
        .collect();
    ensure!(
        environment(&command, false)? == expected,
        "Environment lost UTF16 or empty value"
    );
    command.env_remove("wx_native_synthetic");
    ensure!(
        environment(&command, false)? == "empty=\0\0".encode_utf16().collect::<Vec<_>>(),
        "Windows case-insensitive deletion changed"
    );
    ensure!(
        environment(&Command::new("C:\\synthetic.exe"), false)? == vec![0, 0],
        "Empty environment is not double-NUL"
    );
    Ok(())
}
