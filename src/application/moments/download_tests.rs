use super::*;
use std::{fs, net::TcpListener, thread, time::Instant};

struct ShortStream {
    remaining: usize,
    reads: usize,
    change: Option<std::path::PathBuf>,
}

impl Read for ShortStream {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        assert!(output.len() <= 16 * 1024, "unbounded stream buffer");
        self.reads += 1;
        // First twelve one-byte reads establish the signature, then target capture occurs.
        if self.reads == 13 {
            if let Some(path) = self.change.take() {
                fs::write(path, b"concurrent")?;
            }
        }
        let n = self
            .remaining
            .min(output.len())
            .min(if self.reads <= 12 { 1 } else { 997 });
        output[..n].fill(b'x');
        self.remaining -= n;
        Ok(n)
    }
}

#[test]
fn streamed_publication_short_reads_hidden_name_and_limit_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(root.path()).unwrap();
    let destination = root.path().join(".hidden");
    let actual = root.path().join(".hidden.bin");
    for size in [100, 1024 * 1024] {
        let mut input = ShortStream {
            remaining: size,
            reads: 0,
            change: None,
        };
        let result = publish_response(&mut input, &destination, &guard, size as u64).unwrap();
        assert_eq!(result.actual_filename, ".hidden.bin");
        assert_eq!(result.bytes, size as u64);
        assert_eq!(fs::metadata(&actual).unwrap().len(), size as u64);
    }
    fs::write(&actual, b"old").unwrap();
    for size in [99, 101] {
        let mut input = ShortStream {
            remaining: size,
            reads: 0,
            change: None,
        };
        assert!(publish_response(&mut input, &destination, &guard, 100).is_err());
        assert_eq!(fs::read(&actual).unwrap(), b"old");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn streamed_publication_refuses_target_changed_after_signature() {
    let root = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(root.path()).unwrap();
    let actual = root.path().join("item.bin");
    fs::write(&actual, b"old").unwrap();
    let mut input = ShortStream {
        remaining: 1024,
        reads: 0,
        change: Some(actual.clone()),
    };
    assert!(publish_response(&mut input, &root.path().join("item"), &guard, 2048).is_err());
    assert_eq!(fs::read(&actual).unwrap(), b"concurrent");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

// 完整读取请求头，服务端自身也设置期限，避免失败测试挂死。
fn serve(replies: Vec<(Vec<u8>, Duration)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/media?secret-token",
        listener.local_addr().unwrap()
    );
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        for (reply, delay) in replies {
            let start = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(5) =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    other => panic!("missing loopback request: {other:?}"),
                }
            };
            // Windows 接受的套接字可能继承非阻塞模式，读取期限不能替代模式切换。
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 32 * 1024);
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            requests.push(String::from_utf8(request).unwrap());
            thread::sleep(delay);
            let _ = stream.write_all(&reply);
        }
        requests
    });
    (url, worker)
}

fn response(status: u16, body: &[u8]) -> Vec<u8> {
    let mut reply = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    reply.extend_from_slice(body);
    reply
}

#[test]
fn server_waits_for_delayed_fragmented_headers() {
    let expected = response(200, b"delayed");
    let (url, worker) = serve(vec![(expected.clone(), Duration::ZERO)]);
    let address = url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    thread::sleep(Duration::from_millis(50));
    stream.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    thread::sleep(Duration::from_millis(25));
    stream.write_all(b"Host: localhost\r\n\r\n").unwrap();
    let mut received = Vec::new();
    stream.read_to_end(&mut received).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        ["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]
    );
    assert_eq!(received, expected);
}

fn body(prefix: &[u8]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    bytes.resize(100, 42);
    bytes
}

fn assert_private(error: anyhow::Error) {
    let text = format!("{error:#?} {error:#}");
    for secret in [
        "secret-token",
        "secret-user",
        "secret-pass",
        "127.0.0.1",
        "http://",
    ] {
        assert!(!text.contains(secret), "error leaked URL information");
    }
}

#[test]
fn headers_formats_and_actual_filename() {
    for (prefix, format) in [
        (b"\xff\xd8\xff".as_slice(), Format::Jpg),
        (b"\x89PNG", Format::Png),
        (b"GIF89a", Format::Gif),
        (b"GIF87a", Format::Gif),
        (b"RIFF0000WEBP", Format::Webp),
        (b"0000ftypqt  ", Format::Mov),
        (b"0000ftypQT  ", Format::Mov),
        (b"0000ftypisom", Format::Mp4),
        (b"0000ftypzzzz", Format::Mp4),
        (b"RIFF0000WAVE", Format::Bin),
        (b"GIF7", Format::Bin),
        (b"encrypted", Format::Bin),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let payload = body(prefix);
        let (url, worker) = serve(vec![(response(200, &payload), Duration::ZERO)]);
        let result = download(
            &url,
            &temp.path().join("image"),
            &guard,
            &Options::default(),
        )
        .unwrap();
        let requests = worker.join().unwrap();
        let request = requests[0].to_ascii_lowercase();
        assert!(request.starts_with("get /media?secret-token http/1.1\r\n"));
        assert!(request.contains(&format!(
            "\r\nuser-agent: {}\r\n",
            USER_AGENT.to_ascii_lowercase()
        )));
        assert!(request.contains(&format!("\r\nreferer: {REFERER}\r\n")));
        assert!(!request.contains("authorization:"));
        assert_eq!(result.format, format);
        assert_eq!(result.bytes, 100);
        assert_eq!(
            result.actual_filename,
            format!("image.{}", format.extension())
        );
        assert_eq!(
            fs::read(temp.path().join(result.actual_filename)).unwrap(),
            payload
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[test]
fn suffix_is_preserved_or_appended_and_success_replaces_old_file() {
    for (name, actual) in [
        ("photo", "photo.png"),
        ("photo.JPG", "photo.JPG"),
        ("photo.custom", "photo.custom"),
        (".hidden", ".hidden.png"),
        ("..hidden", "..hidden.png"),
        (".hidden.ext", ".hidden.ext"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        fs::write(temp.path().join(actual), b"old").unwrap();
        let payload = body(b"\x89PNG");
        let (url, worker) = serve(vec![(response(200, &payload), Duration::ZERO)]);
        let outcome = download(&url, &temp.path().join(name), &guard, &Options::default()).unwrap();
        worker.join().unwrap();
        assert_eq!(outcome.actual_filename, actual);
        assert_eq!(outcome.format, Format::Png);
        assert_eq!(fs::read(temp.path().join(actual)).unwrap(), payload);
    }
}

#[test]
fn strict_status_short_truncated_and_limit_failures_preserve_old_files() {
    let mut cases = vec![
        response(204, b""),
        response(404, &body(b"bad")),
        response(201, &body(b"bad")),
        response(200, &[0; 99]),
        response(200, &[0; 101]),
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n".to_vec(),
    ];
    let mut no_length = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    no_length.extend_from_slice(&[0; 101]);
    cases.push(no_length);
    cases.push(format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n65\r\n{}\r\n0\r\n\r\n", "x".repeat(101)).into_bytes());
    for reply in cases {
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let destination = temp.path().join("media.bin");
        fs::write(&destination, b"original").unwrap();
        let (url, worker) = serve(vec![(reply, Duration::ZERO)]);
        assert_private(
            download(
                &url,
                &destination,
                &guard,
                &Options {
                    max_bytes: 100,
                    ..Options::default()
                },
            )
            .unwrap_err(),
        );
        worker.join().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[test]
fn redirect_boundaries_and_headers() {
    for (location, limit, success) in [
        ("/final", 1, true),
        ("/final", 0, false),
        ("file:///secret-token", 5, false),
        ("http://secret-user:secret-pass@127.0.0.1/", 5, false),
    ] {
        let mut replies = vec![(format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes(), Duration::ZERO)];
        if success {
            replies.push((response(200, &body(b"GIF8")), Duration::ZERO));
        }
        let (url, worker) = serve(replies);
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let result = download(
            &url,
            &temp.path().join("media"),
            &guard,
            &Options {
                max_redirects: limit,
                ..Options::default()
            },
        );
        let requests = worker.join().unwrap();
        assert_eq!(result.is_ok(), success);
        if let Err(e) = result {
            assert_private(e);
        }
        for request in requests {
            assert!(request
                .to_ascii_lowercase()
                .contains(&format!("\r\nreferer: {REFERER}\r\n")));
            assert!(request.contains(USER_AGENT));
        }
    }
    // 循环在给定跳数边界停止，而不是直到超时。
    let redirect = b"HTTP/1.1 302 Found\r\nLocation: /media?secret-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (url, worker) = serve(vec![
        (redirect.clone(), Duration::ZERO),
        (redirect, Duration::ZERO),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    assert_private(
        download(
            &url,
            &temp.path().join("media"),
            &guard,
            &Options {
                max_redirects: 1,
                ..Options::default()
            },
        )
        .unwrap_err(),
    );
    assert_eq!(worker.join().unwrap().len(), 2);
}

#[test]
fn timeout_and_invalid_options_urls_are_private_and_preserve_files() {
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let destination = temp.path().join("media.bin");
    fs::write(&destination, b"old").unwrap();
    let (url, worker) = serve(vec![(
        response(200, &body(b"GIF8")),
        Duration::from_millis(350),
    )]);
    let start = Instant::now();
    assert_private(
        download(
            &url,
            &destination,
            &guard,
            &Options {
                timeout: Duration::from_millis(100),
                ..Options::default()
            },
        )
        .unwrap_err(),
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    worker.join().unwrap();
    for url in [
        "file:///secret-token",
        "ftp://127.0.0.1/secret-token",
        "secret-token",
        "http://secret-user:secret-pass@127.0.0.1/",
    ] {
        assert_private(download(url, &destination, &guard, &Options::default()).unwrap_err());
    }
    for options in [
        Options {
            timeout: Duration::ZERO,
            ..Options::default()
        },
        Options {
            max_bytes: 99,
            ..Options::default()
        },
        Options {
            max_bytes: u64::MAX,
            ..Options::default()
        },
        Options {
            max_redirects: usize::MAX,
            ..Options::default()
        },
        Options {
            timeout: Duration::MAX,
            ..Options::default()
        },
    ] {
        assert!(download("http://127.0.0.1:0/", &destination, &guard, &options).is_err());
    }
    assert_eq!(fs::read(&destination).unwrap(), b"old");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn unsafe_paths_and_aliases_do_not_overwrite_source() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("output");
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("nested")).unwrap();
    let source = temp.path().join("source");
    fs::write(&source, b"protected source").unwrap();
    let mut guard = HostOutputGuard::new(&output).unwrap();
    guard.pin_input(&source).unwrap();
    for destination in [
        source.clone(),
        output.clone(),
        output.join("nested/media"),
        output.join("../escape"),
        output.join("NUL"),
        output.join("media:stream"),
        output.join("trailing."),
        output.join("trailing "),
        "relative".into(),
    ] {
        assert!(download(
            "http://127.0.0.1:0/",
            &destination,
            &guard,
            &Options::default()
        )
        .is_err());
    }
    let alias = output.join("alias.bin");
    fs::hard_link(&source, &alias).unwrap();
    assert!(download("http://127.0.0.1:0/", &alias, &guard, &Options::default()).is_err());
    // 无后缀输入必须再校验追加扩展名后的真实目标。
    let (url, worker) = serve(vec![(response(200, &body(b"unknown")), Duration::ZERO)]);
    assert!(download(&url, &output.join("alias"), &guard, &Options::default()).is_err());
    worker.join().unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"protected source");
    assert_eq!(fs::read(&alias).unwrap(), b"protected source");
    assert_eq!(fs::read_dir(&output).unwrap().count(), 2);
}

#[test]
fn slow_body_cannot_reset_total_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/secret-token", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                other => panic!("missing request: {other:?}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 32 * 1024);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
            .unwrap();
        for _ in 0..10 {
            if stream.write_all(&[42; 10]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(60));
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let path = temp.path().join("media.bin");
    fs::write(&path, b"old").unwrap();
    assert_private(
        download(
            &url,
            &path,
            &guard,
            &Options {
                timeout: Duration::from_millis(180),
                ..Options::default()
            },
        )
        .unwrap_err(),
    );
    worker.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn implicit_proxy_environment_is_disabled() {
    // 仅对子进程注入假代理，避免测试并发时修改进程全局环境。
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!(
                "{}::proxy_child",
                module_path!().split_once("::").unwrap().1
            ),
            "--ignored",
        ])
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env("https_proxy", "http://127.0.0.1:1")
        .env("all_proxy", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn publication_failure_preserves_old_file_and_cleans_staging() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let path = temp.path().join("media.bin");
    fs::write(&path, b"old").unwrap();
    // 允许守卫读取，但禁止发布时删除或替换目标。
    let _pinned = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let (url, worker) = serve(vec![(response(200, &body(b"unknown")), Duration::ZERO)]);
    assert_private(download(&url, &path, &guard, &Options::default()).unwrap_err());
    worker.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
#[ignore = "由父测试使用隔离代理环境执行"]
fn proxy_child() {
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let (url, worker) = serve(vec![(response(200, &body(b"GIF8")), Duration::ZERO)]);
    let result = download(
        &url,
        &temp.path().join("media"),
        &guard,
        &Options::default(),
    );
    worker.join().unwrap();
    assert!(result.is_ok());
}
