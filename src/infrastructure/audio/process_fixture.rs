//! Standalone synthetic FFmpeg-shaped helper, compiled only by process_tests.
use std::{
    io::Write,
    time::{Duration, Instant},
};

fn main() {
    let executable = std::env::current_exe().unwrap();
    let mode = executable.file_stem().unwrap().to_str().unwrap();
    let target = std::env::args_os().last().unwrap();
    if mode == "parent_swap" {
        let parent = std::path::Path::new(&target).parent().unwrap();
        assert!(
            std::fs::rename(parent, parent.with_file_name("moved-output")).is_err(),
            "encoder staging parent was not locked"
        );
    }
    std::fs::write(&target, b"synthetic encoded audio").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match mode {
            "success" | "parent_swap" => return,
            "failure" => {
                eprintln!("SYNTHETIC_PRIVATE_KEY");
                std::process::exit(7);
            }
            "flood" => {
                std::io::stdout().write_all(&[b'x'; 8192]).unwrap();
                std::io::stderr().write_all(&[b'y'; 8192]).unwrap();
            }
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
