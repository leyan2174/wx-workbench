use super::*;
use std::{net::TcpListener, thread};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic";
const JPG: &[u8] = b"\xff\xd8\xffsynthetic";
const GIF: &[u8] = b"GIF89asynthetic";
const WEBP: &[u8] = b"RIFF1234WEBPsynthetic";
fn fixture() -> (tempfile::TempDir, HostOutputGuard) {
    let dir = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(dir.path()).unwrap();
    (dir, guard)
}
fn no_stream(_: &str, _: usize) -> Result<Vec<u8>> {
    panic!("明文或无 key 不应初始化 WASM")
}

#[test]
fn python_ast_url_oracles() {
    let output = std::process::Command::new("python")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sns-album-images/oracle.py"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (i, case) in cases.as_array().unwrap().iter().enumerate() {
        let url = case["url"].as_str().unwrap();
        let token = case["token"].as_str().unwrap();
        assert_eq!(
            serde_json::to_value(sns_image_url_candidates(url, token)).unwrap(),
            case["candidates"],
            "synthetic case {i}"
        );
        assert_eq!(
            fix_sns_image_url(url, token),
            case["fixed"].as_str().unwrap(),
            "synthetic case {i}"
        );
    }
}

#[test]
fn exact_magic_and_real_extension() {
    for (data, ext) in [(JPG, "jpg"), (PNG, "png"), (GIF, "gif"), (WEBP, "webp")] {
        let (dir, guard) = fixture();
        let result = save_image(data, "post.fake", &guard).unwrap();
        assert_eq!(result.filename, format!("post.{ext}"));
        assert_eq!(result.bytes, data.len() as u64);
        assert_eq!(fs::read(dir.path().join(&result.filename)).unwrap(), data);
    }
    for bad in [
        b"".as_slice(),
        b"GIF",
        b"\x89PNG",
        b"RIFF1234WAVE",
        b"\xff\xd8",
    ] {
        assert_eq!(detect(bad), None);
    }
}

#[test]
fn existing_order_and_header_validation() {
    let (dir, guard) = fixture();
    fs::write(dir.path().join("post.jpg"), PNG).unwrap();
    fs::write(dir.path().join("post.png"), PNG).unwrap();
    fs::write(dir.path().join("post.gif"), GIF).unwrap();
    let outcome = download_with(
        "",
        "",
        "",
        "post.any",
        &guard,
        |_| panic!("已有图不下载"),
        no_stream,
    )
    .unwrap();
    assert_eq!(outcome.filename, "post.png");
    assert_eq!(outcome.source.as_str(), "existing");
    assert_eq!(outcome.bytes, PNG.len() as u64);
    fs::write(dir.path().join("post.jpg"), JPG).unwrap();
    assert_eq!(
        reuse_existing_image("post", &guard)
            .unwrap()
            .unwrap()
            .filename,
        "post.jpg"
    );
    for (ext, data) in [("jpg", JPG), ("png", PNG), ("gif", GIF), ("webp", WEBP)] {
        fs::write(dir.path().join(format!("only_{ext}.{ext}")), data).unwrap();
        assert_eq!(
            reuse_existing_image(&format!("only_{ext}"), &guard)
                .unwrap()
                .unwrap()
                .filename,
            format!("only_{ext}.{ext}")
        );
    }
}

#[test]
fn plaintext_and_ordered_fallback() {
    let (_dir, guard) = fixture();
    let mut visited = Vec::new();
    let result = download_with(
        "HTTP://example.invalid/150",
        "secret-key",
        "token",
        "p",
        &guard,
        |url| {
            visited.push(url.to_owned());
            if visited.len() == 1 {
                Err(ImageError::HttpStatus)
            } else {
                Ok(PNG.to_vec())
            }
        },
        no_stream,
    )
    .unwrap();
    assert_eq!(
        visited,
        [
            "https://example.invalid/0?token=token&idx=1",
            "https://example.invalid/150?token=token&idx=1"
        ]
    );
    assert_eq!(result.source.as_str(), "remote");
}

#[test]
fn encrypted_xor_and_magic_recheck() {
    let (dir, guard) = fixture();
    let encrypted: Vec<u8> = WEBP.iter().map(|b| b ^ 0xa5).collect();
    let result = download_with(
        "https://example.invalid/0",
        " 42 ",
        "",
        "p",
        &guard,
        |_| Ok(encrypted.clone()),
        |key, n| {
            assert_eq!(key, "42");
            assert_eq!(n, WEBP.len());
            Ok(vec![0xa5; n])
        },
    )
    .unwrap();
    assert_eq!(result.source.as_str(), "remote_decrypted");
    assert_eq!(fs::read(dir.path().join("p.webp")).unwrap(), WEBP);
    let error = download_with(
        "https://example.invalid/0",
        "42",
        "",
        "bad",
        &guard,
        |_| Ok(vec![1; 20]),
        |_, n| Ok(vec![0; n]),
    )
    .unwrap_err();
    assert_eq!(error.errors, [ImageError::UnsupportedFormat]);
    assert!(!dir.path().join("bad.jpg").exists());
}

#[test]
fn missing_keys_bad_stream_and_lazy_engine() {
    let (_dir, guard) = fixture();
    for key in ["", "0", " 0 ", "\x1c\t"] {
        assert_eq!(
            download_with(
                "https://example.invalid/0",
                key,
                "",
                "p",
                &guard,
                |_| Ok(vec![1; 20]),
                no_stream
            )
            .unwrap_err()
            .errors,
            [ImageError::UnsupportedFormat]
        );
    }
    assert_eq!(
        download_with(
            "https://example.invalid/0",
            "42",
            "",
            "p",
            &guard,
            |_| Ok(vec![1; 20]),
            |_, _| Ok(vec![1; 19])
        )
        .unwrap_err()
        .errors,
        [ImageError::Decrypt]
    );
    assert_eq!(
        download_with(
            "https://example.invalid/0",
            "42",
            "",
            "p",
            &guard,
            |_| Ok(vec![1; 20]),
            |_, _| Err(ImageError::EngineUnavailable)
        )
        .unwrap_err()
        .errors,
        [ImageError::EngineUnavailable]
    );
    fs::write(guard.output_root().join("p.png"), PNG).unwrap();
    assert_eq!(
        download_sns_image("", "42", "", "p", &guard, || panic!("已有图不获取 engine"))
            .unwrap()
            .source,
        ImageSource::Existing
    );
}

#[test]
fn size_missing_url_and_redacted_failures() {
    let (_dir, guard) = fixture();
    assert_eq!(
        download_with(" &Tab; ", "", "", "p", &guard, |_| panic!(), no_stream)
            .unwrap_err()
            .errors,
        [ImageError::MissingUrl]
    );
    for n in [0, MAX_BYTES + 1] {
        let error = download_with(
            "https://example.invalid/150?secret-url=private",
            "secret-key",
            "secret-token",
            "p",
            &guard,
            |_| Ok(vec![0; n]),
            no_stream,
        )
        .unwrap_err();
        assert_eq!(
            error.errors,
            [ImageError::ResponseSize, ImageError::ResponseSize]
        );
        let text = format!("{error} {error:?}");
        for secret in ["example", "secret", "private"] {
            assert!(!text.contains(secret));
        }
    }
    let mut data = vec![0; MAX_BYTES];
    data[..PNG.len()].copy_from_slice(PNG);
    assert_eq!(
        download_with(
            "https://example.invalid/0",
            "",
            "",
            "limit",
            &guard,
            |_| Ok(data.clone()),
            no_stream
        )
        .unwrap()
        .bytes,
        MAX_BYTES as u64
    );
}

#[test]
fn no_overwrite_and_unsafe_paths() {
    let (dir, guard) = fixture();
    fs::write(dir.path().join("p.png"), b"source-sentinel").unwrap();
    assert_eq!(save_image(PNG, "p", &guard), Err(ImageError::Output));
    assert_eq!(
        fs::read(dir.path().join("p.png")).unwrap(),
        b"source-sentinel"
    );
    for name in ["../escape", "a/b", "a\\b", "C:escape", "", "..", "CON"] {
        assert_eq!(save_image(PNG, name, &guard), Err(ImageError::Output));
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    let source = tempfile::tempdir().unwrap();
    let mut protected = HostOutputGuard::new(source.path()).unwrap();
    assert!(protected.protect(source.path()).is_err());
}

#[test]
fn reject_hardlinked_output_without_touching_source() {
    let (dir, guard) = fixture();
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("source.png");
    fs::write(&path, PNG).unwrap();
    fs::hard_link(&path, dir.path().join("p.png")).unwrap();
    assert_eq!(reuse_existing_image("p", &guard), Err(ImageError::Output));
    assert_eq!(save_image(PNG, "p", &guard), Err(ImageError::Output));
    assert_eq!(fs::read(path).unwrap(), PNG);
}

fn server(response: Vec<u8>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/image", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let _ = socket.write_all(&response);
        String::from_utf8(request).unwrap()
    });
    (url, handle)
}

#[test]
fn transport_loopback_headers_status_size_scheme() {
    let client = client().unwrap();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        PNG.len()
    )
    .into_bytes();
    response.extend_from_slice(PNG);
    let (url, handle) = server(response);
    assert_eq!(fetch(&client, &url).unwrap(), PNG);
    let request = handle.join().unwrap().to_ascii_lowercase();
    for header in [
        "user-agent: micromessenger client\r\n",
        "accept: */*\r\n",
        "referer: https://mp.weixin.qq.com/\r\n",
    ] {
        assert!(request.contains(header));
    }
    for (response, error) in [
        (
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned(),
            ImageError::HttpStatus,
        ),
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_BYTES + 1
            ),
            ImageError::ResponseSize,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned(),
            ImageError::ResponseSize,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\na".to_owned(),
            ImageError::BodyRead,
        ),
    ] {
        let (url, handle) = server(response.into_bytes());
        assert_eq!(fetch(&client, &url), Err(error));
        handle.join().unwrap();
    }
    for url in [
        "file:///private",
        "ftp://example.invalid/image",
        "https://user:password@example.invalid/",
    ] {
        assert_eq!(fetch(&client, url), Err(ImageError::InvalidUrl));
    }
    // 只测试 transport 的 HTTP；生产入口始终先把 HTTP 升级为 HTTPS。
    assert!(fix_sns_image_url("HTTP://127.0.0.1:1/image", "").starts_with("https://"));
}

#[test]
fn real_runtime_stream_and_limit() {
    let engine =
        VideoRuntime::bundled(super::super::video_runtime::RuntimeLimits::default()).unwrap();
    let (_dir, guard) = fixture();
    let mut plain = vec![0; 128 * 1024 + 1];
    plain[..PNG.len()].copy_from_slice(PNG);
    let stream = engine.keystream("42", plain.len()).unwrap();
    let encrypted: Vec<u8> = plain.iter().zip(stream).map(|(a, b)| a ^ b).collect();
    assert_eq!(
        download_with(
            "https://example.invalid/0",
            "42",
            "",
            "real",
            &guard,
            |_| Ok(encrypted.clone()),
            |key, n| engine.keystream(key, n).map_err(|_| ImageError::Decrypt)
        )
        .unwrap()
        .source,
        ImageSource::RemoteDecrypted
    );
    assert_eq!(
        download_with(
            "https://example.invalid/0",
            "42",
            "",
            "large",
            &guard,
            |_| Ok(vec![1; MAX_BYTES + 1]),
            |key, n| engine.keystream(key, n).map_err(|_| ImageError::Decrypt)
        )
        .unwrap_err()
        .errors,
        [ImageError::ResponseSize]
    );
}

#[test]
fn transport_redirects_are_bounded_and_reject_other_schemes() {
    let client = client().unwrap();
    let (url, handle) = server(
        b"HTTP/1.1 302 Found\r\nLocation: file:///private\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    assert_eq!(fetch(&client, &url), Err(ImageError::HttpStatus));
    handle.join().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/loop", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let deadline = std::time::Instant::now();
        let mut count = 0;
        while count < 6 && deadline.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        socket.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    count += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
        count
    });
    assert_eq!(fetch(&client, &url), Err(ImageError::Http));
    assert_eq!(handle.join().unwrap(), 6);
}

#[test]
fn transport_unknown_length_is_bounded() {
    let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    response.resize(response.len() + MAX_BYTES + 1, 1);
    let (url, handle) = server(response);
    assert_eq!(
        fetch(&client().unwrap(), &url),
        Err(ImageError::ResponseSize)
    );
    handle.join().unwrap();
}

#[test]
fn redirected_request_keeps_fixed_headers() {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        PNG.len()
    )
    .into_bytes();
    response.extend_from_slice(PNG);
    let (target, target_handle) = server(response);
    let (url, first_handle) = server(format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes());
    assert_eq!(
        fetch(&client().unwrap(), &format!("{url}?token=synthetic-secret")).unwrap(),
        PNG
    );
    first_handle.join().unwrap();
    let request = target_handle.join().unwrap().to_ascii_lowercase();
    assert!(request.contains("referer: https://mp.weixin.qq.com/\r\n"));
    assert!(request.contains("user-agent: micromessenger client\r\n"));
    assert!(request.contains("accept: */*\r\n"));
    assert!(!request.contains("synthetic-secret"));
}

#[test]
fn transport_total_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/slow", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        thread::sleep(Duration::from_secs(11));
    });
    let start = std::time::Instant::now();
    assert_eq!(fetch(&client().unwrap(), &url), Err(ImageError::Http));
    assert!(start.elapsed() >= Duration::from_secs(9));
    assert!(start.elapsed() < Duration::from_secs(13));
    handle.join().unwrap();
}

#[test]
fn fallback_after_bad_magic_decrypt_and_publish_failures() {
    for first_error in [
        ImageError::UnsupportedFormat,
        ImageError::Decrypt,
        ImageError::Output,
    ] {
        let (dir, guard) = fixture();
        if first_error == ImageError::Output {
            fs::write(dir.path().join("p.jpg"), b"invalid-existing").unwrap();
        }
        let mut count = 0;
        let result = download_with(
            "https://example.invalid/150",
            if first_error == ImageError::Decrypt {
                "42"
            } else {
                "0"
            },
            "",
            "p",
            &guard,
            |_| {
                count += 1;
                Ok(if count == 2 {
                    PNG.to_vec()
                } else if first_error == ImageError::Output {
                    JPG.to_vec()
                } else {
                    vec![0; 20]
                })
            },
            |_, _| Err(ImageError::Decrypt),
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(result.filename, "p.png");
    }
}
