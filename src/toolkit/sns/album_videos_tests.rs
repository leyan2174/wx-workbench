use super::super::video_runtime::RuntimeLimits;
use super::*;
use std::{
    net::{TcpListener, TcpStream},
    thread,
    time::Instant,
};

fn fixture() -> (tempfile::TempDir, HostOutputGuard) {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let guard = HostOutputGuard::new(&output).unwrap();
    (root, guard)
}
fn plain(n: usize) -> Vec<u8> {
    let mut bytes: Vec<_> = (0..n).map(|i| (i % 251) as u8).collect();
    bytes[..12].copy_from_slice(b"\0\0\0\x18ftypisom");
    bytes
}
fn unused() -> Result<&'static VideoRuntime> {
    panic!("plain path initialized WASM")
}

#[test]
fn remote_target_changed_during_request_is_not_replaced() {
    let (_root, guard) = fixture();
    let path = guard.output_root().join("video.mp4");
    fs::write(&path, b"old").unwrap();
    let changed = path.clone();
    let (url, handle) = server(move |mut socket| {
        fs::write(changed, b"concurrent").unwrap();
        let bytes = plain(VIDEO_PREFIX_BYTES + 100);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        let _ = socket.write_all(&bytes);
    });
    let result = download_video(&url, "", "video", &guard, unused);
    handle.join().unwrap();
    assert_eq!(result, Err(VideoError::Output));
    clean(&guard, b"concurrent");
}

#[test]
fn stream_copy_bounds_buffers_and_preserves_typed_errors() {
    struct Input(usize);
    impl Read for Input {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            assert!(buffer.len() <= 64 * 1024);
            let n = self.0.min(buffer.len()).min(997);
            buffer[..n].fill(7);
            self.0 -= n;
            Ok(n)
        }
    }
    let size = 4 * 1024 * 1024;
    assert_eq!(
        copy_tail(&mut Input(size), &mut std::io::sink(), 0, size as u64),
        Ok(size as u64)
    );
    assert_eq!(
        copy_tail(&mut Input(101), &mut std::io::sink(), 0, 100),
        Err(VideoError::Size)
    );
    assert_eq!(
        publication_error(anyhow::Error::new(VideoError::BodyRead)),
        VideoError::BodyRead
    );
    assert_eq!(
        publication_error(anyhow::Error::new(VideoError::Size)),
        VideoError::Size
    );
}

// 监听及读写均有期限；Windows 接受套接字后显式恢复阻塞模式。
fn server(action: impl FnOnce(TcpStream) + Send + 'static) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/video?token=synthetic-secret",
        listener.local_addr().unwrap()
    );
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        assert!(request.len() < 8192 && Instant::now() < deadline);
                        let mut b = [0];
                        socket.read_exact(&mut b).unwrap();
                        request.push(b[0]);
                    }
                    let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                    assert!(request.contains("user-agent: micromessenger client\r\n"));
                    assert!(request.contains("accept: */*\r\n"));
                    assert!(!request.contains("referer:"));
                    action(socket);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "accept deadline");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    (url, handle)
}
fn body(bytes: Vec<u8>, fragmented: bool) -> (String, thread::JoinHandle<()>) {
    server(move |mut socket| {
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        if fragmented {
            for chunk in bytes.chunks(997) {
                if socket.write_all(chunk).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        } else {
            let _ = socket.write_all(&bytes);
        }
    })
}
fn clean(guard: &HostOutputGuard, expected: &[u8]) {
    assert_eq!(
        fs::read(guard.output_root().join("video.mp4")).unwrap(),
        expected
    );
    assert_eq!(fs::read_dir(guard.output_root()).unwrap().count(), 1);
}

#[test]
fn plain_short_reads_and_replacement_do_not_load_wasm() {
    let (_root, guard) = fixture();
    fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
    let bytes = plain(VIDEO_PREFIX_BYTES + 11003);
    let (url, handle) = body(bytes.clone(), true);
    let result = download_video(&url, "", "video", &guard, unused).unwrap();
    handle.join().unwrap();
    assert_eq!(
        result,
        outcome(
            Path::new("video.mp4"),
            VideoSource::Remote,
            true,
            bytes.len() as u64
        )
    );
    clean(&guard, &bytes);
    assert_eq!(
        reuse_existing_video("video.any", &guard)
            .unwrap()
            .unwrap()
            .source
            .as_str(),
        "existing"
    );
}

#[test]
fn wasm_oracle_prefix_and_tail_with_fragmented_network() {
    let runtime = VideoRuntime::bundled(RuntimeLimits {
        input_bytes: VIDEO_PREFIX_BYTES,
        ..Default::default()
    })
    .unwrap();
    for size in [19, VIDEO_PREFIX_BYTES + 32779] {
        let (_root, guard) = fixture();
        let expected = plain(size);
        let mut encrypted = expected.clone();
        let mask = runtime
            .keystream("123456789", size.min(VIDEO_PREFIX_BYTES))
            .unwrap();
        for (b, k) in encrypted.iter_mut().zip(mask) {
            *b ^= k;
        }
        let (url, handle) = body(encrypted, true);
        let mut calls = 0;
        let result = download_video(&url, "123456789", "video", &guard, || {
            calls += 1;
            Ok(&runtime)
        })
        .unwrap();
        handle.join().unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result.bytes, size as u64);
        clean(&guard, &expected);
    }
}

#[test]
fn encrypted_video_larger_than_runtime_input_limit_streams_tail() {
    let runtime = VideoRuntime::bundled(RuntimeLimits {
        input_bytes: VIDEO_PREFIX_BYTES,
        ..Default::default()
    })
    .unwrap();
    let (_root, guard) = fixture();
    let expected = plain(25 * 1024 * 1024 + 37);
    let mut encrypted = expected.clone();
    let mask = runtime.keystream("123", VIDEO_PREFIX_BYTES).unwrap();
    for (b, k) in encrypted.iter_mut().zip(mask) {
        *b ^= k;
    }
    let (url, handle) = body(encrypted, false);
    download_video(&url, "123", "video", &guard, || Ok(&runtime)).unwrap();
    handle.join().unwrap();
    clean(&guard, &expected);
}

#[test]
fn complete_partial_cache_and_source_preservation() {
    let (root, guard) = fixture();
    let expected = plain(VIDEO_PREFIX_BYTES + 7);
    for (ext, complete) in [("MP4", true), ("tmp", false)] {
        let source = root.path().join(format!("cache.{ext}"));
        fs::write(&source, &expected).unwrap();
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        if !complete {
            assert!(copy_cached_video(&source, "video", &guard, false)
                .unwrap()
                .is_none());
        }
        let copied = copy_cached_video(&source, "video", &guard, true)
            .unwrap()
            .unwrap();
        assert_eq!(copied.complete, complete);
        assert_eq!(copied.source.as_str(), "cache");
        assert_eq!(fs::read(&source).unwrap(), expected);
        assert_eq!(
            fs::metadata(guard.output_root().join("video.mp4"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        clean(&guard, &expected);
    }
}

#[test]
fn cache_missing_invalid_and_alias_rejected() {
    let (root, guard) = fixture();
    assert!(
        copy_cached_video(&root.path().join("missing.mp4"), "video", &guard, false)
            .unwrap()
            .is_none()
    );
    let source = root.path().join("source.mp4");
    fs::write(&source, b"not a real mp4 header").unwrap();
    assert!(copy_cached_video(&source, "video", &guard, false)
        .unwrap()
        .is_none());
    fs::write(&source, plain(32)).unwrap();
    let alias = root.path().join("alias.mp4");
    fs::hard_link(&source, &alias).unwrap();
    assert_eq!(
        copy_cached_video(&source, "video", &guard, false),
        Err(VideoError::Source)
    );
    fs::remove_file(alias).unwrap();
    let target = guard.output_root().join("video.mp4");
    fs::hard_link(&source, &target).unwrap();
    assert!(reuse_existing_video("video", &guard).is_err());
    assert!(copy_cached_video(&source, "video", &guard, false).is_err());
    assert!(download_video("http://127.0.0.1:1", "", "video", &guard, unused).is_err());
    assert_eq!(fs::read(&source).unwrap(), plain(32));
    fs::remove_file(target).unwrap();
    fs::write(guard.output_root().join("source.mp4"), plain(32)).unwrap();
    assert!(copy_cached_video(
        &guard.output_root().join("source.mp4"),
        "video",
        &guard,
        false
    )
    .is_err());
}

#[test]
fn reuse_missing_invalid_and_oversize() {
    let (_root, guard) = fixture();
    assert!(reuse_existing_video("video", &guard).unwrap().is_none());
    let path = guard.output_root().join("video.mp4");
    fs::write(&path, b"bad").unwrap();
    assert!(reuse_existing_video("video", &guard).unwrap().is_none());
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(MAX_VIDEO_BYTES + 1).unwrap();
    drop(file);
    assert!(reuse_existing_video("video", &guard).unwrap().is_none());
}

#[test]
fn status_declared_size_empty_and_truncation_preserve_old() {
    for header in [
        "HTTP/1.1 500 Failed\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Range: bytes 0-11/24\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 2147483649\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 190000\r\n\r\n",
    ] {
        let (_root, guard) = fixture();
        fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
        let (url, handle) = server(move |mut s| {
            let _ = s.write_all(header.as_bytes());
            let _ = s.write_all(&plain(VIDEO_PREFIX_BYTES + 111));
        });
        let error = download_video(&url, "", "video", &guard, unused).unwrap_err();
        handle.join().unwrap();
        assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        clean(&guard, b"old");
    }
}

#[test]
fn unknown_length_limit_and_exact_limit() {
    for size in [VIDEO_PREFIX_BYTES + 17, VIDEO_PREFIX_BYTES + 18] {
        let (_root, guard) = fixture();
        fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
        let expected = plain(size);
        let bytes = expected.clone();
        let (url, handle) = server(move |mut s| {
            s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .unwrap();
            let _ = s.write_all(&bytes);
        });
        let result = download_with(
            &url,
            "",
            "video",
            &guard,
            unused,
            (VIDEO_PREFIX_BYTES + 17) as u64,
            TIMEOUT,
        );
        handle.join().unwrap();
        if size == VIDEO_PREFIX_BYTES + 17 {
            assert!(result.is_ok());
            clean(&guard, &expected);
        } else {
            assert_eq!(result, Err(VideoError::Size));
            clean(&guard, b"old");
        }
    }
}

#[test]
fn key_errors_invalid_header_and_output_names_preserve_old() {
    let (_root, guard) = fixture();
    fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
    for name in [
        "../video",
        "CON",
        "video:ads",
        "video/part",
        "video.",
        "video ",
        "",
    ] {
        assert!(download_video("http://127.0.0.1:1", "", name, &guard, unused).is_err());
    }
    for key in ["", "synthetic-key"] {
        let (url, handle) = body(vec![42; 33], false);
        let result = download_video(&url, key, "video", &guard, || {
            Err(VideoError::EngineUnavailable)
        });
        handle.join().unwrap();
        assert_eq!(
            result,
            Err(if key.is_empty() {
                VideoError::MissingKey
            } else {
                VideoError::EngineUnavailable
            })
        );
        clean(&guard, b"old");
    }
    let runtime = VideoRuntime::bundled(RuntimeLimits::default()).unwrap();
    let (url, handle) = body(vec![42; 33], false);
    assert_eq!(
        download_video(&url, "123", "video", &guard, || Ok(&runtime)),
        Err(VideoError::InvalidMp4)
    );
    handle.join().unwrap();
    clean(&guard, b"old");
}

#[test]
fn url_and_redirect_boundaries() {
    let (_root, guard) = fixture();
    for url in [
        "file:///C:/secret",
        "ftp://host/file",
        "http://user:secret@host/",
        "http://host/a#secret",
        "http://host/\nsecret",
    ] {
        assert_eq!(
            download_video(url, "", "video", &guard, unused),
            Err(VideoError::InvalidUrl)
        );
    }
    for location in ["file:///C:/secret", "http://user:secret@localhost/video"] {
        let (url, handle) = server(move |mut s| {
            write!(
                s,
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
        });
        assert!(matches!(
            download_video(&url, "", "video", &guard, unused),
            Err(VideoError::Http | VideoError::HttpStatus)
        ));
        handle.join().unwrap();
    }
    let expected = plain(23);
    let (target, second) = body(expected.clone(), false);
    let (url, first) = server(move |mut s| {
        write!(
            s,
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
    });
    download_video(&url, "", "video", &guard, unused).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    clean(&guard, &expected);
}

#[test]
fn bounded_network_timeout_cleans_staging() {
    let (_root, guard) = fixture();
    fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
    let (url, handle) = server(|mut s| {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 200000\r\n\r\n")
            .unwrap();
        let _ = s.write_all(&plain(VIDEO_PREFIX_BYTES));
        thread::sleep(Duration::from_millis(300));
    });
    let start = Instant::now();
    assert!(download_with(
        &url,
        "",
        "video",
        &guard,
        unused,
        MAX_VIDEO_BYTES,
        Duration::from_millis(100)
    )
    .is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    handle.join().unwrap();
    clean(&guard, b"old");
}

#[test]
fn production_limits_and_counter_boundary() {
    assert_eq!(MAX_VIDEO_BYTES, 2147483648);
    assert_eq!(TIMEOUT, Duration::from_secs(30));
    assert_eq!(VIDEO_PREFIX_BYTES, 131072);
    let mut output = Vec::new();
    assert_eq!(
        copy_tail(
            &mut &b"x"[..],
            &mut output,
            MAX_VIDEO_BYTES - 1,
            MAX_VIDEO_BYTES
        ),
        Ok(MAX_VIDEO_BYTES)
    );
    assert_eq!(
        copy_tail(
            &mut &b"x"[..],
            &mut output,
            MAX_VIDEO_BYTES,
            MAX_VIDEO_BYTES
        ),
        Err(VideoError::Size)
    );
}

#[test]
fn source_handle_blocks_mutation_until_released() {
    let (root, _guard) = fixture();
    let source = root.path().join("source.mp4");
    fs::write(&source, plain(32)).unwrap();
    let pinned = open_video(&source).unwrap().unwrap();
    assert!(OpenOptions::new().write(true).open(&source).is_err());
    assert!(fs::remove_file(&source).is_err());
    drop(pinned);
    fs::write(&source, plain(64)).unwrap();
}

#[test]
fn chunked_complete_and_truncated_body() {
    for complete in [true, false] {
        let (_root, guard) = fixture();
        fs::write(guard.output_root().join("video.mp4"), b"old").unwrap();
        let expected = plain(VIDEO_PREFIX_BYTES + 11);
        let bytes = expected.clone();
        let (url, handle) = server(move |mut socket| {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .unwrap();
            write!(socket, "{:x}\r\n", bytes.len()).unwrap();
            socket.write_all(&bytes).unwrap();
            socket.write_all(b"\r\n").unwrap();
            if complete {
                socket.write_all(b"0\r\n\r\n").unwrap();
            }
        });
        let result = download_video(&url, "", "video", &guard, unused);
        handle.join().unwrap();
        if complete {
            assert!(result.is_ok());
            clean(&guard, &expected);
        } else {
            assert_eq!(result, Err(VideoError::BodyRead));
            clean(&guard, b"old");
        }
    }
}
