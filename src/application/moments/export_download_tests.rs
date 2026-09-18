use super::*;
use rusqlite::Connection;
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

// 合成服务器只监听回环；期限和 Drop 负责回收失败测试的后台线程。
struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Vec<String>>>,
}

impl Server {
    fn new(routes: Vec<(&str, u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: BTreeMap<_, _> = routes
            .into_iter()
            .map(|(path, status, body)| (path.to_owned(), (status, body)))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let start = Instant::now();
            let mut requests = Vec::new();
            while !stopped.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("loopback accept failed: {e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    assert!(request.len() < 32 * 1024);
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                let target = request.split_whitespace().nth(1).unwrap().to_owned();
                let path = target.split('?').next().unwrap();
                let (status, body) = routes.get(path).cloned().unwrap_or((404, Vec::new()));
                requests.push(target);
                let header = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
            requests
        });
        Self {
            base,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn body(prefix: &[u8]) -> Vec<u8> {
    let mut body = prefix.to_vec();
    body.resize(100, 42);
    body
}

fn media(kind: &str, url: &str, thumb: &str, width: u32) -> String {
    format!("<media><type>{kind}</type><url>{}</url><thumb>{}</thumb><size width=\"{width}\" height=\"{width}\" /></media>", escape(url), escape(thumb))
}

fn database(root: &Path, posts: &[Vec<String>]) -> PathBuf {
    let path = root.join("synthetic.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);")
        .unwrap();
    for (i, media) in posts.iter().enumerate() {
        // 同秒帖子同时覆盖尾号去重和媒体索引稳定性。
        let xml = format!("<root><LocalExtraInfo><nickname>Tester</nickname></LocalExtraInfo><TimelineObject><id>{i}</id><username>test-user</username><createTime>1700000000</createTime><ContentObject><type>1</type><mediaList>{}</mediaList></ContentObject></TimelineObject></root>", media.join(""));
        db.execute(
            "INSERT INTO SnsTimeLine VALUES (?1, 'test-user', ?2)",
            rusqlite::params![i as i64, xml],
        )
        .unwrap();
    }
    path
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn options() -> ExportOptions {
    ExportOptions {
        export_time: Some(1700000000),
        ..ExportOptions::default()
    }
}

fn limits() -> DownloadOptions {
    DownloadOptions {
        timeout: Duration::from_secs(2),
        max_bytes: 100,
        ..DownloadOptions::default()
    }
}

fn assert_files(report: &ExportReport) {
    let unique: BTreeSet<_> = report.files.iter().collect();
    assert_eq!(unique.len(), report.files.len());
    for path in &report.files {
        assert!(path.is_file(), "reported file missing: {}", path.display());
        assert!(!path.to_string_lossy().contains(".wx-sns-"));
    }
}

#[test]
fn explicit_none_authorization_never_requests_network() {
    let server = Server::new(vec![("/remote", 200, body(b"GIF8"))]);
    let temp = tempfile::tempdir().unwrap();
    let db = database(
        temp.path(),
        &[vec![media("2", &server.url("/remote"), "", 0)]],
    );
    let cache_keys = CacheKeys::default();
    let index = cache::build_cache_index(
        &cache::CacheRoots::default(),
        &cache_keys,
        cache::CacheLimits::default(),
    )
    .unwrap();
    let recovery = CacheRecovery {
        index: &index,
        keys: &cache_keys,
        verify: &|| Ok(()),
    };
    for mode in 0..4 {
        let output = temp.path().join(format!("out-{mode}"));
        let report = match mode {
            0 => export_database_with_media(&db, None, &output, &options(), None, None),
            1 => export_database_with_media(&db, None, &output, &options(), None, None),
            2 => export_database_with_media(&db, None, &output, &options(), Some(&recovery), None),
            _ => export_database_with_media(&db, None, &output, &options(), None, None),
        }
        .unwrap();
        assert_eq!(
            (report.media_downloaded, report.media_download_failed),
            (0, 0)
        );
        let root = output.join("Tester/SNS");
        let summary = json(&root.join("timeline.json"));
        assert!(summary["posts"][0]["media"][0].get("local_file").is_none());
        let html = fs::read_to_string(root.join("timeline.html")).unwrap();
        assert!(!html.contains("<img") && !html.contains("<video"));
        assert_files(&report);
    }
    assert!(server.finish().is_empty());
}

#[test]
fn no_cache_downloads_actual_formats_and_publishes_consistent_references() {
    let cases = [
        ("/misleading.jpg", b"\x89PNG".as_slice(), "png"),
        ("/jpeg", b"\xff\xd8\xff", "jpg"),
        ("/gif", b"GIF8", "gif"),
        ("/webp", b"RIFF0000WEBP", "webp"),
        ("/mov", b"0000ftypqt  ", "mov"),
        ("/mp4", b"0000ftypisom", "mp4"),
        ("/unknown.png", b"unknown", "bin"),
    ];
    let server = Server::new(
        cases
            .iter()
            .map(|(path, prefix, _)| (*path, 200, body(prefix)))
            .collect(),
    );
    let temp = tempfile::tempdir().unwrap();
    let db = database(
        temp.path(),
        &[cases
            .iter()
            .map(|(path, _, _)| media("99", &server.url(path), "", 0))
            .collect()],
    );
    let output = temp.path().join("out");
    let report =
        export_database_with_media(&db, None, &output, &options(), None, Some(&limits())).unwrap();
    assert_eq!(
        (
            report.media_recovered,
            report.media_downloaded,
            report.media_failed,
            report.media_missing
        ),
        (7, 7, 0, 0)
    );
    let root = output.join("Tester/SNS");
    let summary = json(&root.join("timeline.json"));
    let audit = json(&root.join("_media_recovery.json"));
    let post = json(&root.join(audit[0]["post_file"].as_str().unwrap()));
    assert_eq!(summary["posts"][0], post);
    let html = fs::read_to_string(root.join("timeline.html")).unwrap();
    for (i, (_, prefix, ext)) in cases.iter().enumerate() {
        let reference = &post["media"][i];
        let name = reference["local_file"].as_str().unwrap();
        assert!(name.ends_with(&format!("_{i}.{ext}")));
        assert_eq!(reference["download_format"], *ext);
        assert_eq!(fs::read(root.join(name)).unwrap(), body(prefix));
        assert_eq!(
            audit[0]["recovery"]["media"][i]["reference"]["local_file"],
            name
        );
        assert_eq!(audit[0]["recovery"]["media"][i]["bytes"], 100);
        let tag = if *ext == "bin" {
            format!("<a href=\"{name}\" download>")
        } else if matches!(*ext, "mp4" | "mov") {
            format!("<video src=\"{name}\"")
        } else {
            format!("<img src=\"{name}\"")
        };
        assert!(html.contains(&tag));
    }
    assert!(!html.contains("src=\"http"));
    assert!(html.contains("img-src 'self'; media-src 'self'"));
    assert_files(&report);
    // 仍然拒绝重跑，并且预检失败不得新增网络请求或改变旧汇总。
    let before = fs::read(root.join("timeline.json")).unwrap();
    assert!(
        export_database_with_media(&db, None, &output, &options(), None, Some(&limits())).is_err()
    );
    assert_eq!(fs::read(root.join("timeline.json")).unwrap(), before);
    assert_eq!(server.finish().len(), 7);
}

#[test]
fn failed_publication_verification_preserves_existing_timeline_and_media() {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("source");
    fs::create_dir(&source_root).unwrap();
    let db = database(
        &source_root,
        &[vec![media("2", "https://synthetic.invalid/image", "", 16)]],
    );
    let (index, keys, _, _) = image_cache(temp.path());
    let output = temp.path().join("out");
    let publication = TimelinePublication {
        flat_cache: false,
        source_kind: "snapshot".into(),
        source_id: "synthetic-verification-boundary".into(),
        policy: crate::infrastructure::output_tree::ExistingPolicy::Update,
        inputs: vec![db.clone()],
    };
    let calls = Cell::new(0);
    let allow = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    let report = export_database_with_publication(
        &db,
        None,
        &output,
        &options(),
        Some(&CacheRecovery {
            index: &index,
            keys: &keys,
            verify: &allow,
        }),
        None,
        &publication,
    )
    .unwrap();
    assert_eq!(report.media_recovered, 1);
    let successful_checks = calls.get();
    assert!(successful_checks > 1);
    let before = tree(&output);
    assert!(before.keys().any(|path| path.ends_with("timeline.json")));
    assert!(before
        .keys()
        .any(|path| path.extension().is_some_and(|ext| ext == "png")));

    let connection = Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE SnsTimeLine SET content = replace(content, '1700000000', '1700000060')",
            [],
        )
        .unwrap();
    drop(connection);
    let reject = || -> anyhow::Result<()> {
        calls.set(calls.get() + 1);
        anyhow::bail!("synthetic broker revision changed before publication")
    };
    let error = export_database_with_publication(
        &db,
        None,
        &output,
        &options(),
        Some(&CacheRecovery {
            index: &index,
            keys: &keys,
            verify: &reject,
        }),
        None,
        &publication,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("synthetic broker revision changed"));
    assert_eq!(calls.get(), successful_checks + 1);
    assert_eq!(
        tree(&output),
        before,
        "failed verification must not publish new summaries, media or staging files"
    );

    export_database_with_publication(
        &db,
        None,
        &output,
        &options(),
        Some(&CacheRecovery {
            index: &index,
            keys: &keys,
            verify: &allow,
        }),
        None,
        &publication,
    )
    .unwrap();
    assert!(calls.get() > successful_checks + 2);
    assert_ne!(
        tree(&output),
        before,
        "the update must be observable when verification succeeds"
    );
}

fn image_cache(root: &Path) -> (CacheIndex, CacheKeys, Vec<u8>, PathBuf) {
    let storage = root.join("cache");
    let month = storage.join("2023-11");
    fs::create_dir_all(&month).unwrap();
    let mut plain = body(b"\x89PNG\r\n\x1a\n");
    plain[16..20].copy_from_slice(&16u32.to_be_bytes());
    plain[20..24].copy_from_slice(&16u32.to_be_bytes());
    let source = month.join("synthetic-image");
    fs::write(&source, plain.iter().map(|b| b ^ 0x88).collect::<Vec<_>>()).unwrap();
    let keys = CacheKeys::default();
    let index = cache::build_cache_index(
        &cache::CacheRoots {
            xwechat: None,
            file_storage_sns: Some(storage),
        },
        &keys,
        cache::CacheLimits::default(),
    )
    .unwrap();
    assert_eq!(index.images().len(), 1);
    (index, keys, plain, source)
}

#[test]
fn partial_cache_is_first_and_only_unrecovered_indices_download() {
    let server = Server::new(vec![
        ("/cached", 200, body(b"WRONG")),
        ("/missing", 200, body(b"GIF8")),
        ("/video", 200, body(b"0000ftypisom")),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let (index, keys, plain, source) = image_cache(temp.path());
    let before = fs::read(&source).unwrap();
    let db = database(
        temp.path(),
        &[vec![
            media("2", &server.url("/cached"), "", 16),
            media("2", &server.url("/missing"), "", 99),
            media("6", &server.url("/video"), "", 0),
        ]],
    );
    let output = temp.path().join("out");
    let report = export_database_with_media(
        &db,
        None,
        &output,
        &options(),
        Some(&CacheRecovery {
            index: &index,
            keys: &keys,
            verify: &|| Ok(()),
        }),
        Some(&limits()),
    )
    .unwrap();
    assert_eq!(
        (
            report.media_recovered,
            report.media_downloaded,
            report.media_missing,
            report.media_failed
        ),
        (3, 2, 0, 0)
    );
    let root = output.join("Tester/SNS");
    let summary = json(&root.join("timeline.json"));
    let media = &summary["posts"][0]["media"];
    let cached = media[0]["local_file"].as_str().unwrap();
    assert!(cached.starts_with("images/") && cached.ends_with("_0.png"));
    assert_eq!(fs::read(root.join(cached)).unwrap(), plain);
    assert_eq!(media[0]["image_source"], "cache");
    assert!(media[1]["local_file"].as_str().unwrap().ends_with("_1.gif"));
    assert!(media[2]["local_file"].as_str().unwrap().ends_with("_2.mp4"));
    assert_eq!(fs::read(source).unwrap(), before);
    assert_files(&report);
    assert_eq!(server.finish(), vec!["/missing", "/video"]);
}

#[test]
fn failed_cache_can_download_without_double_counting_final_status() {
    let server = Server::new(vec![("/replacement", 200, body(b"GIF8"))]);
    let temp = tempfile::tempdir().unwrap();
    let (index, keys, _, source) = image_cache(temp.path());
    fs::write(source, b"changed after indexing").unwrap();
    let db = database(
        temp.path(),
        &[vec![media("2", &server.url("/replacement"), "", 16)]],
    );
    let output = temp.path().join("out");
    let report = export_database_with_media(
        &db,
        None,
        &output,
        &options(),
        Some(&CacheRecovery {
            index: &index,
            keys: &keys,
            verify: &|| Ok(()),
        }),
        Some(&limits()),
    )
    .unwrap();
    assert_eq!(
        (
            report.media_recovered,
            report.media_downloaded,
            report.media_failed
        ),
        (1, 1, 0)
    );
    assert!(report.warnings.iter().any(|s| s.contains("media 0")));
    assert_eq!(report.media_download_failed, 0);
    assert_files(&report);
    assert_eq!(server.finish(), vec!["/replacement"]);
}

#[test]
fn failures_continue_and_thumb_is_only_used_when_url_is_empty() {
    let server = Server::new(vec![
        ("/bad", 404, body(b"bad")),
        ("/no-content", 204, Vec::new()),
        ("/short", 200, vec![0; 99]),
        ("/large", 200, vec![0; 101]),
        ("/thumb", 200, body(b"GIF8")),
        ("/good", 200, body(b"\x89PNG")),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let db = database(
        temp.path(),
        &[
            vec![
                media(
                    "2",
                    &server.url("/bad?secret-token"),
                    &server.url("/must-not-fallback"),
                    0,
                ),
                media("2", &server.url("/no-content"), "", 0),
                media("2", &server.url("/short"), "", 0),
                media("2", &server.url("/large"), "", 0),
                media("2", "http://secret-user:secret-pass@127.0.0.1/", "", 0),
                media("2", "", &server.url("/thumb"), 0),
                media("2", "", "", 0),
            ],
            vec![media("2", &server.url("/good"), "", 0)],
        ],
    );
    let output = temp.path().join("out");
    let report =
        export_database_with_media(&db, None, &output, &options(), None, Some(&limits())).unwrap();
    assert_eq!(
        (
            report.posts,
            report.media_downloaded,
            report.media_download_failed,
            report.media_failed,
            report.media_missing
        ),
        (2, 2, 5, 5, 1)
    );
    let diagnostics = format!("{report:?}");
    for secret in ["secret-token", "secret-user", "secret-pass", "http://"] {
        assert!(!diagnostics.contains(secret));
    }
    let root = output.join("Tester/SNS");
    let summary = json(&root.join("timeline.json"));
    for i in [0, 1, 2, 3, 4, 6] {
        assert!(summary["posts"][0]["media"][i].get("local_file").is_none());
    }
    assert!(summary["posts"][0]["media"][5]["local_file"]
        .as_str()
        .unwrap()
        .ends_with("_5.gif"));
    assert!(summary["posts"][1]["media"][0]["local_file"]
        .as_str()
        .unwrap()
        .ends_with("_0.png"));
    let audit = json(&root.join("_media_recovery.json"));
    assert_ne!(audit[0]["post_file"], audit[1]["post_file"]);
    assert_files(&report);
    assert_eq!(
        server.finish(),
        vec![
            "/bad?secret-token",
            "/no-content",
            "/short",
            "/large",
            "/thumb",
            "/good"
        ]
    );
}
