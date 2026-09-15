use super::*;
use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut};
use std::{io::Write, net::TcpListener};

const MD5: &str = "0123456789abcdef0123456789abcdef";
const KEY: &str = "000102030405060708090a0b0c0d0e0f";

#[test]
#[cfg(windows)]
fn resource_failure_and_bad_material_have_distinct_stages() {
    use crate::adapters::wechat::emoticons::CatalogSource;
    use crate::business::emoticons::Source;
    use crate::business::media::Stage;
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch(include_str!(
        "../../../tests/fixtures/emoticons-catalog/schema.sql"
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO kNonStoreEmoticonTable VALUES(?1,'','file:///synthetic-unavailable','','')",
        [MD5],
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(output.path()).unwrap();
    let source = CatalogSource::from_path(&database).unwrap();
    let reference = source.catalog().unwrap().items[0].reference.clone();
    let error = export_from(&source, &reference, &guard, &Default::default(), &[]).unwrap_err();
    assert_eq!(error.stage, Stage::Download);
    let (url, server) = serve(vec![response(b"synthetic-ciphertext")]);
    conn.execute(
        "UPDATE kNonStoreEmoticonTable SET aes_key='not-hex',encrypt_url=?1",
        [url],
    )
    .unwrap();
    let source = CatalogSource::from_path(&database).unwrap();
    let reference = source.catalog().unwrap().items[0].reference.clone();
    let error = export_from(&source, &reference, &guard, &Default::default(), &[]).unwrap_err();
    server.join().unwrap();
    assert_eq!(error.stage, Stage::Decode);
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
#[cfg(windows)]
fn catalog_reference_and_legacy_cache_results_are_not_hash_proofs() {
    use crate::adapters::wechat::emoticons::CatalogSource;
    use crate::business::emoticons::{Materialization, Source};
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("catalog.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch(include_str!(
        "../../../tests/fixtures/emoticons-catalog/schema.sql"
    ))
    .unwrap();
    conn.execute("INSERT INTO kNonStoreEmoticonTable VALUES(?1,'secret','file:///must-not-be-fetched','','')", [MD5]).unwrap();
    let source = CatalogSource::from_path(&database).unwrap();
    let reference = source.catalog().unwrap().items[0].reference.clone();
    let output = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(output.path()).unwrap();
    fs::write(
        output.path().join(format!("{MD5}.gif")),
        b"legacy-cache-not-a-hash-proof",
    )
    .unwrap();
    let (result, _) = export_from(&source, &reference, &guard, &Default::default(), &[]).unwrap();
    assert_eq!(result.materialization, Materialization::LegacyCache);
    drop(source);
    let another = CatalogSource::from_path(&database).unwrap();
    let error = export_from(&another, &reference, &guard, &Default::default(), &[]).unwrap_err();
    assert_eq!(
        error.failure,
        crate::business::media::Failure::StaleEvidence
    );
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn shared_publication_protects_existing_bin_and_cleans_temporary_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(format!("{MD5}.bin"));
    fs::write(&path, b"protected-old").unwrap();
    let guard = HostOutputGuard::new(root.path()).unwrap();
    let (url, server) = serve(vec![response(b"new-unknown-format")]);
    let error = download_with_protection(
        MD5,
        &EmojiInfo {
            cdn_url: url,
            ..Default::default()
        },
        &guard,
        &Default::default(),
        &[path.clone()],
    )
    .unwrap_err();
    server.join().unwrap();
    assert_eq!(
        error
            .downcast_ref::<crate::business::media::Error>()
            .unwrap()
            .stage,
        crate::business::media::Stage::Publication
    );
    assert_eq!(fs::read(&path).unwrap(), b"protected-old");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn shared_publication_never_overwrites_an_image_arriving_during_fetch() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(format!("{MD5}.gif"));
    let guard = HostOutputGuard::new(root.path()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/image", listener.local_addr().unwrap());
    let destination = path.clone();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "synthetic client did not connect"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("synthetic listener failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        fs::write(destination, b"concurrent-image").unwrap();
        stream.write_all(&response(b"GIF89anew")).unwrap();
    });
    assert!(download(
        MD5,
        &EmojiInfo {
            cdn_url: url,
            ..Default::default()
        },
        &guard,
        &Default::default()
    )
    .is_err());
    server.join().unwrap();
    assert_eq!(fs::read(path).unwrap(), b"concurrent-image");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn hevc_zero_deadline_uses_managed_runner_without_leaking_scratch() {
    let root = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(root.path()).unwrap();
    let options = DownloadOptions {
        timeout: Duration::ZERO,
        ffmpeg: Some(root.path().join("never-start.exe")),
        ..Default::default()
    };
    let error = convert_hevc_to_jpeg(&[VPS, b"synthetic"].concat(), &guard, &options).unwrap_err();
    assert!(format!("{error:#}").contains("deadline expired"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

fn serve(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/emoji?secret-query",
        listener.local_addr().unwrap()
    );
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        let start = Instant::now();
        for response in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(10) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    result => panic!("loopback request missing: {result:?}"),
                }
            };
            // Windows 接受的套接字可能继承监听器的非阻塞模式。
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            let mut used = 0;
            // TCP 可拆分请求头；收到结束标志后才响应，并保留固定大小上限。
            loop {
                assert!(used < request.len(), "loopback request header too large");
                let read = stream.read(&mut request[used..]).unwrap();
                assert!(read > 0, "loopback request ended before headers");
                used += read;
                if request[..used].windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(&response);
        }
    });
    (url, thread)
}
fn response(body: &[u8]) -> Vec<u8> {
    let mut result = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    result.extend_from_slice(body);
    result
}
fn encrypted(data: &[u8]) -> Vec<u8> {
    let key: Vec<u8> = (0..16).collect();
    cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &key)
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(data)
}

#[test]
fn loopback_server_waits_for_delayed_request() {
    let expected = response(b"GIF89adelayed");
    let (url, server) = serve(vec![expected.clone()]);
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
    // 连接建立不代表 HTTP 请求已到达，服务端必须等待可读数据。
    std::thread::sleep(Duration::from_millis(50));
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut received = Vec::new();
    stream.read_to_end(&mut received).unwrap();
    server.join().unwrap();
    assert_eq!(received, expected);
}

#[test]
fn aes_padding_and_errors() {
    for data in [b"GIF89ahello".as_slice(), &[42; 16], &[0; 32]] {
        assert_eq!(decrypt(&encrypted(data), KEY).unwrap(), data);
    }
    let key: Vec<u8> = (0..16).collect();
    for tail in [0, 17, 2] {
        let mut raw = [99; 16];
        raw[15] = tail;
        let enc = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &key)
            .unwrap()
            .encrypt_padded_vec_mut::<NoPadding>(&raw);
        assert_eq!(decrypt(&enc, KEY).unwrap(), raw);
    }
    assert!(decrypt(b"bad", KEY).is_err());
    assert!(decrypt(&[0; 16], "sensitive-invalid-key")
        .unwrap_err()
        .to_string()
        .find("sensitive")
        .is_none());
}

#[test]
fn fromhex_whitespace_requires_byte_boundaries() {
    let data = encrypted(b"GIF89atest");
    for whitespace in [" ", "\t", "\n", "\r", "\x0b", "\x0c", " \t\r\n\x0b\x0c"] {
        let pairs = KEY
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| std::str::from_utf8(pair).unwrap())
            .collect::<Vec<_>>();
        let key = format!("{whitespace}{}{whitespace}", pairs.join(whitespace));
        assert_eq!(decrypt(&data, &key).unwrap(), b"GIF89atest");
        let broken = format!("{}{whitespace}{}", &KEY[..1], &KEY[1..]);
        assert!(decrypt(&data, &broken).is_err());
    }
    for key in [
        format!("{KEY}\u{a0}"),
        format!("{KEY}00"),
        KEY[..30].into(),
        KEY[..31].into(),
        "".into(),
    ] {
        assert!(decrypt(&data, &key).is_err());
    }
    assert!(decrypt(b"", KEY).unwrap().is_empty());
}

#[test]
#[cfg(windows)]
fn direct_failure_falls_back_but_short_nonempty_does_not() {
    for first in [
        response(b""),
        b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let (url, server) = serve(vec![first, response(&encrypted(b"GIF89afallback"))]);
        let info = EmojiInfo {
            cdn_url: url.clone(),
            encrypt_url: url,
            aes_key: KEY.into(),
            ..Default::default()
        };
        assert_eq!(
            download(MD5, &info, &guard, &Default::default())
                .unwrap()
                .filename,
            format!("{MD5}.gif")
        );
        server.join().unwrap();
    }
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let (url, server) = serve(vec![response(b"GIF")]);
    let info = EmojiInfo {
        cdn_url: url.clone(),
        encrypt_url: url,
        aes_key: KEY.into(),
        ..Default::default()
    };
    assert!(download(MD5, &info, &guard, &Default::default()).is_err());
    server.join().unwrap();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
#[cfg(windows)]
fn bin_retry_failure_preserves_old_file_and_rejects_alias() {
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let path = temp.path().join(format!("{MD5}.bin"));
    fs::write(&path, b"old").unwrap();
    for body in [b"WXGFbroken".as_slice(), b"WXGFchanged"] {
        let (url, server) = serve(vec![response(body)]);
        let got = download(
            MD5,
            &EmojiInfo {
                cdn_url: url,
                ..Default::default()
            },
            &guard,
            &DownloadOptions {
                ffmpeg: None,
                ..Default::default()
            },
        )
        .unwrap();
        server.join().unwrap();
        assert!(got.conversion_fallback && !got.cached);
        assert_eq!(fs::read(&path).unwrap(), body);
    }
    let info = EmojiInfo {
        encrypt_url: "http://127.0.0.1:0/private?secret-query".into(),
        aes_key: "sensitive-key".into(),
        ..Default::default()
    };
    let error = download(MD5, &info, &guard, &Default::default()).unwrap_err();
    assert!(!format!("{error:#?}").contains("secret-query"));
    assert!(!format!("{error:#?}").contains("sensitive-key"));
    assert_eq!(fs::read(&path).unwrap(), b"WXGFchanged");
    let source = tempfile::NamedTempFile::new().unwrap();
    fs::remove_file(&path).unwrap();
    fs::hard_link(source.path(), &path).unwrap();
    let (url, server) = serve(vec![response(b"unknown")]);
    assert!(download(
        MD5,
        &EmojiInfo {
            cdn_url: url,
            ..Default::default()
        },
        &guard,
        &Default::default()
    )
    .is_err());
    server.join().unwrap();
    assert_eq!(fs::metadata(source.path()).unwrap().len(), 0);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn hevc_missing_stream_and_converter_leave_no_scratch() {
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let opts = DownloadOptions {
        ffmpeg: Some(temp.path().join("missing-ffmpeg.exe")),
        ..Default::default()
    };
    for data in [
        b"WXGFbroken".to_vec(),
        [b"WXGF".as_slice(), VPS, b"broken"].concat(),
    ] {
        assert!(convert_hevc_to_jpeg(&data, &guard, &opts).is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
#[cfg(windows)]
#[ignore = "需要宿主安装带 libx265 编码器的 FFmpeg"]
fn real_ffmpeg_first_frame_and_cleanup() {
    let fixture = tempfile::tempdir().unwrap();
    let input = fixture.path().join("two-frames.h265");
    let result = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=32x32:r=1",
            "-frames:v",
            "2",
            "-c:v",
            "libx265",
            "-x265-params",
            "log-level=error",
            "-f",
            "hevc",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(result.status.success(), "合成 HEVC 失败");
    let data = [b"WXGFsynthetic".as_slice(), &fs::read(input).unwrap()].concat();
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let (url, server) = serve(vec![response(&data)]);
    let got = download(
        MD5,
        &EmojiInfo {
            cdn_url: url,
            ..Default::default()
        },
        &guard,
        &Default::default(),
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(got.filename, format!("{MD5}.jpg"));
    assert!(got.converted);
    assert!(!got.conversion_fallback);
    let jpeg = fs::read(temp.path().join(got.filename)).unwrap();
    assert!(jpeg.starts_with(b"\xff\xd8\xff") && jpeg.ends_with(b"\xff\xd9"));
    assert_eq!(jpeg.windows(2).filter(|w| *w == b"\xff\xd8").count(), 1);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    // 真实解码失败同样必须清理输入和输出临时文件。
    assert!(convert_hevc_to_jpeg(
        &[b"WXGF".as_slice(), VPS, b"broken"].concat(),
        &guard,
        &Default::default()
    )
    .is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn signatures_and_annex_order() {
    for (data, ext) in [
        (b"\xff\xd8\xffx".as_slice(), "jpg"),
        (b"\x89PNG", "png"),
        (b"GIFx", "gif"),
        (b"RIFF", "webp"),
        (b"WXGF", "hevc"),
        (b"nope", "bin"),
    ] {
        assert_eq!(detect(data), ext);
    }
    let mut data = vec![0; 250];
    data.extend_from_slice(VPS);
    assert_eq!(detect(&data), "hevc");
    data.insert(0, 0);
    assert_eq!(detect(&data), "bin");
    let data = [SPS, b"padding", VPS].concat();
    assert_eq!(find(&data, VPS).or_else(|| find(&data, SPS)), Some(13));
    assert_eq!(find(SPS, VPS).or_else(|| find(SPS, SPS)), Some(0));
}

#[test]
#[cfg(windows)]
fn direct_encrypted_unknown_and_fallback_publish() {
    for (direct, body, expected, fallback) in [
        (true, b"GIF89atest".as_slice(), "gif", false),
        (false, b"\x89PNGtest", "png", false),
        (true, b"RIFFtest", "webp", false),
        (true, b"unknown", "bin", false),
        (true, b"WXGFbroken", "bin", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let replies = if direct {
            vec![response(body)]
        } else {
            vec![response(b""), response(&encrypted(body))]
        };
        let (url, server) = serve(replies);
        let info = EmojiInfo {
            cdn_url: url.clone(),
            encrypt_url: url,
            aes_key: KEY.into(),
            ..Default::default()
        };
        let opts = DownloadOptions {
            ffmpeg: None,
            ..Default::default()
        };
        let got = download(MD5, &info, &guard, &opts).unwrap();
        server.join().unwrap();
        assert_eq!(got.filename, format!("{MD5}.{expected}"));
        assert_eq!(got.conversion_fallback, fallback);
        assert_eq!(fs::read(temp.path().join(got.filename)).unwrap(), body);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[test]
#[cfg(windows)]
fn cache_order_aliases_and_bin_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let guard = HostOutputGuard::new(temp.path()).unwrap();
    let info = EmojiInfo {
        cdn_url: "file:///secret?query".into(),
        ..Default::default()
    };
    for ext in ["webp", "jpg", "png", "gif"] {
        fs::write(temp.path().join(format!("{MD5}.{ext}")), ext).unwrap();
        let got = download(MD5, &info, &guard, &Default::default()).unwrap();
        assert!(got.cached);
        assert_eq!(got.filename, format!("{MD5}.{ext}"));
    }
    assert!(download("../escape", &info, &guard, &Default::default()).is_err());
    let gif = temp.path().join(format!("{MD5}.gif"));
    fs::remove_file(&gif).unwrap();
    fs::hard_link(temp.path().join(format!("{MD5}.png")), &gif).unwrap();
    assert!(download(MD5, &info, &guard, &Default::default()).is_err());
    for entry in fs::read_dir(temp.path()).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    fs::write(temp.path().join(format!("{MD5}.bin")), b"original").unwrap();
    let (url, server) = serve(vec![response(b"unknown")]);
    let got = download(
        MD5,
        &EmojiInfo {
            cdn_url: url,
            ..Default::default()
        },
        &guard,
        &Default::default(),
    )
    .unwrap();
    assert!(!got.cached);
    server.join().unwrap();
    assert_eq!(
        fs::read(temp.path().join(format!("{MD5}.bin"))).unwrap(),
        b"unknown"
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn network_limits_redirects_protocol_and_sanitization() {
    let opts = DownloadOptions {
        max_bytes: 4,
        timeout: Duration::from_millis(150),
        ..Default::default()
    };
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(opts.timeout)
        .build()
        .unwrap();
    for url in ["file:///secret?query", "ftp://127.0.0.1/key", "not a url"] {
        assert!(fetch(&client, url, &opts).is_err());
    }
    for reply in [
        response(b"too large"),
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ntoo large".to_vec(),
        b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
    ] {
        let (url, server) = serve(vec![reply]);
        let err = fetch(&client, &url, &opts).unwrap_err();
        assert!(!format!("{err:?}").contains("secret-query"));
        server.join().unwrap();
    }
}

#[test]
#[cfg(windows)]
fn redirect_download_and_rejection() {
    for (location, limit, success) in [
        ("/final", 1, true),
        ("/final", 0, false),
        ("file:///private", 5, false),
    ] {
        let reply = format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
        let replies = if success {
            vec![reply, response(b"GIF89a")]
        } else {
            vec![reply]
        };
        let (url, server) = serve(replies);
        let temp = tempfile::tempdir().unwrap();
        let guard = HostOutputGuard::new(temp.path()).unwrap();
        let result = download(
            MD5,
            &EmojiInfo {
                cdn_url: url,
                ..Default::default()
            },
            &guard,
            &DownloadOptions {
                max_redirects: limit,
                ..Default::default()
            },
        );
        assert_eq!(result.is_ok(), success);
        server.join().unwrap();
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            usize::from(success)
        );
    }
}
