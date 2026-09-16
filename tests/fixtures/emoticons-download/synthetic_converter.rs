//! Synthetic executable for process-boundary tests; never invokes a media engine.
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

// The timeout mode intentionally leaves its descendant running so the parent
// test can prove that the production Job Object reaps the whole process tree.
#[allow(clippy::zombie_processes)]
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--child") {
        let root = PathBuf::from(&args[2]);
        fs::write(root.join("child-pid"), std::process::id().to_string()).unwrap();
        for sequence in 0u64.. {
            fs::write(root.join("heartbeat"), sequence.to_string()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    let output = PathBuf::from(args.last().unwrap());
    let root = output.parent().unwrap().parent().unwrap();
    fs::write(root.join("converter-started"), b"ready").unwrap();
    let executable = std::env::current_exe().unwrap();
    let mode = executable.file_stem().unwrap().to_string_lossy();
    if mode == "timeout" {
        let mut command = Command::new(&executable);
        command
            .arg("--child")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("heartbeat").exists() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not start");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::write(root.join("descendant-ready"), b"ready").unwrap();
        loop {
            std::thread::sleep(Duration::from_millis(100));
        }
    } else if mode == "flood" {
        let bytes = [b'x'; 8192];
        loop {
            if std::io::stdout().write_all(&bytes).is_err() {
                break;
            }
        }
    } else {
        fs::write(output, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    }
}
