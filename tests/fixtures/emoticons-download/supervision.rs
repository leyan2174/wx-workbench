#![cfg(windows)]
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    time::{Duration, Instant},
};

fn run(root: &Path, mode: &str) -> emoticons_download_tests::download::Downloaded {
    let executable_dir = tempfile::tempdir().unwrap();
    let executable = executable_dir.path().join(format!("{mode}.exe"));
    fs::copy(env!("CARGO_BIN_EXE_synthetic-converter"), &executable).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/synthetic", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "synthetic HTTP client did not connect"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("synthetic HTTP failure: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        let body = b"WXGF\x00\x00\x00\x01\x40\x01synthetic";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    let result =
        emoticons_download_tests::convert_download(root, &url, &executable, Duration::from_secs(8));
    server.join().unwrap();
    result.unwrap()
}

fn no_scratch(root: &Path) {
    assert!(fs::read_dir(root)
        .unwrap()
        .all(|entry| !entry.unwrap().file_type().unwrap().is_dir()));
}

#[test]
fn successful_managed_converter_publishes_conversion_result() {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), "success");
    assert!(result.converted && !result.conversion_fallback);
    assert_eq!(
        fs::read(root.path().join(result.filename)).unwrap(),
        [0xff, 0xd8, 0xff, 0xd9]
    );
    assert!(root.path().join("converter-started").is_file());
    no_scratch(root.path());
}

#[test]
fn timed_out_converter_reaps_an_actually_started_descendant() {
    use windows::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), "timeout");
    assert!(result.conversion_fallback && !result.converted);
    assert!(
        root.path().join("descendant-ready").is_file(),
        "timeout happened before the descendant started"
    );
    let pid: u32 = fs::read_to_string(root.path().join("child-pid"))
        .unwrap()
        .parse()
        .unwrap();
    if let Ok(handle) = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        let state = unsafe { WaitForSingleObject(handle, 1000) };
        unsafe {
            CloseHandle(handle).unwrap();
        }
        assert_eq!(state, WAIT_OBJECT_0, "synthetic descendant still running");
    }
    let heartbeat = fs::read(root.path().join("heartbeat")).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(fs::read(root.path().join("heartbeat")).unwrap(), heartbeat);
    no_scratch(root.path());
}

#[test]
fn output_limit_failure_keeps_original_binary_and_cleans_scratch() {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), "flood");
    assert!(result.conversion_fallback && !result.converted);
    assert!(fs::read(root.path().join(result.filename))
        .unwrap()
        .starts_with(b"WXGF"));
    assert!(root.path().join("converter-started").is_file());
    no_scratch(root.path());
}
