//! Bounded CLI capture. The caller's RuntimeCleanup owns the longer-lived daemon.
use std::{
    fs,
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

struct OwnedCli(Child);
impl Drop for OwnedCli {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn output(command: &mut Command, directory: &Path, timeout: Duration) -> Output {
    // A helper Job would terminate the daemon when the one-shot CLI exits.
    let stdout = tempfile::NamedTempFile::new_in(directory).unwrap();
    let stderr = tempfile::NamedTempFile::new_in(directory).unwrap();
    let mut child = OwnedCli(
        command
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(stdout.reopen().unwrap())
            .stderr(stderr.reopen().unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        let oversized = [stdout.path(), stderr.path()]
            .iter()
            .any(|path| fs::metadata(path).unwrap().len() > 1024 * 1024);
        assert!(
            !oversized && Instant::now() < deadline,
            "CLI exceeded its time/output budget"
        );
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: fs::read(stdout.path()).unwrap(),
        stderr: fs::read(stderr.path()).unwrap(),
    }
}
