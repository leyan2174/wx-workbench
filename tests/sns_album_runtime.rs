//! 合成加密数据库驱动真实 Feed、daemon 和相册 CLI，不读取本机微信资料。

use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha512;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const USER: &str = "synthetic-album-author";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic-album-image";
const MP4: &[u8] = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isomsynthetic-video";
const COUNTERS: &[&str] = &[
    "posts",
    "album_posts",
    "image_ok",
    "image_existing",
    "image_cache",
    "image_remote",
    "image_missing",
    "video_total",
    "video_ok",
    "video_complete",
    "video_existing",
    "video_cache",
    "video_remote",
    "video_partial_cache",
    "video_missing",
    "video_skipped",
];

struct Fixture {
    root: tempfile::TempDir,
    profiles: Vec<PathBuf>,
    originals: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::Builder::new()
                .prefix("wx-sns-album-runtime-")
                .tempdir()
                .unwrap(),
            profiles: vec![],
            originals: vec![],
        }
    }

    fn account(&mut self, name: &str, posts: &[(i64, i64, &str, &str, String)]) -> PathBuf {
        let profile = self.root.path().join(name);
        fs::create_dir_all(profile.join("db_storage/contact")).unwrap();
        fs::create_dir_all(profile.join("db_storage/sns")).unwrap();
        let plain = profile.join("contact-plain.db");
        let conn = database(&plain);
        conn.execute_batch("CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER);").unwrap();
        for user in [USER, "synthetic-other-author"] {
            conn.execute("INSERT INTO contact VALUES (?1, ?1, '', 0)", [user])
                .unwrap();
        }
        drop(conn);
        encrypt(&plain, &profile.join("db_storage/contact/contact.db"));
        let plain = profile.join("sns-plain.db");
        let conn = database(&plain);
        conn.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);")
            .unwrap();
        for (tid, timestamp, author, content, media) in posts {
            let xml = format!("<TimelineObject><id>{tid}</id><username>{author}</username><createTime>{timestamp}</createTime><contentDesc>{content}</contentDesc><ContentObject><mediaList>{media}</mediaList></ContentObject></TimelineObject>");
            conn.execute(
                "INSERT INTO SnsTimeLine VALUES (?1,?2,?3)",
                rusqlite::params![tid, author, xml],
            )
            .unwrap();
        }
        drop(conn);
        encrypt(&plain, &profile.join("db_storage/sns/sns.db"));
        fs::write(
            profile.join("all_keys.json"),
            json!({"contact/contact.db":"11".repeat(32),"sns/sns.db":"11".repeat(32)}).to_string(),
        )
        .unwrap();
        fs::write(
            profile.join("config.json"),
            json!({"db_dir":"db_storage","keys_file":"all_keys.json","decrypted_dir":"decrypted"})
                .to_string(),
        )
        .unwrap();
        for relative in [
            "db_storage/contact/contact.db",
            "db_storage/sns/sns.db",
            "all_keys.json",
            "config.json",
        ] {
            let path = profile.join(relative);
            self.originals.push((path.clone(), fs::read(path).unwrap()));
        }
        self.profiles.push(profile.clone());
        profile
    }

    fn run(&self, profile: &Path, args: &[&str]) -> Output {
        let empty_path = self.root.path().join("empty-path");
        fs::create_dir_all(&empty_path).unwrap();
        Command::new(env!("CARGO_BIN_EXE_wx"))
            .args(args)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env("WX_CLI_CONFIG", profile.join("config.json"))
            .env("WX_CLI_HOME", self.root.path().join("shared-runtime"))
            .env(
                "WX_WECHAT_DECRYPT_PYTHON",
                self.root.path().join("nonexistent-python.exe"),
            )
            .env("PATH", empty_path)
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .current_dir(self.root.path())
            .output()
            .unwrap()
    }

    fn album(&self, profile: &Path, output: &Path, extra: &[&str]) -> Value {
        let mut args = vec!["sns-album", USER, "--output-dir", output.to_str().unwrap()];
        args.extend_from_slice(extra);
        let summary = success(self.run(profile, &args));
        let disk = read_json(&output.join("export_summary.json"));
        assert_eq!(summary, disk, "stdout 必须与落盘摘要一致");
        assert_eq!(summary["user"], USER);
        for (key, file) in [
            ("output_dir", ""),
            ("timeline_json", "timeline.json"),
            ("html", "timeline.html"),
        ] {
            assert_eq!(
                fs::canonicalize(summary[key].as_str().unwrap()).unwrap(),
                fs::canonicalize(output.join(file)).unwrap()
            );
        }
        self.unchanged();
        summary
    }

    fn unchanged(&self) {
        for (path, original) in &self.originals {
            assert_eq!(
                &fs::read(path).unwrap(),
                original,
                "源文件被修改: {}",
                path.display()
            );
        }
    }
}

#[path = "support/bootstrap.rs"]
mod bootstrap;

impl Drop for Fixture {
    fn drop(&mut self) {
        // 始终传入本测试的独立配置与运行目录，禁止停止其他账号后台。
        for profile in &self.profiles {
            let _ = self.run(profile, &["daemon", "stop"]);
        }
        drop(bootstrap::RuntimeCleanup(
            self.root.path().join("shared-runtime"),
        ));
    }
}

fn database(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA page_size=4096;").unwrap();
    let mut reserve: i32 = 80;
    // 为 SQLCipher 的 IV 和 HMAC 预留每页尾部空间。
    assert_eq!(
        unsafe {
            rusqlite::ffi::sqlite3_file_control(
                conn.handle(),
                c"main".as_ptr(),
                rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
                (&mut reserve as *mut i32).cast(),
            )
        },
        rusqlite::ffi::SQLITE_OK
    );
    conn
}

fn encrypt(plain: &Path, output: &Path) {
    let plain = fs::read(plain).unwrap();
    assert_eq!(plain[20], 80);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mac_salt: Vec<_> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut mac_key = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&key, &mac_salt, 2, &mut mac_key);
    let mut encrypted = Vec::new();
    for (index, plain) in plain.chunks_exact(4096).enumerate() {
        let start = if index == 0 { 16 } else { 0 };
        let mut page = [0u8; 4096];
        if index == 0 {
            page[..16].copy_from_slice(&salt);
        }
        let iv = [0x55u8; 16];
        let data = cbc::Encryptor::<aes::Aes256>::new((&key).into(), (&iv).into())
            .encrypt_padded_vec_mut::<NoPadding>(&plain[start..4016]);
        page[start..4016].copy_from_slice(&data);
        page[4016..4032].copy_from_slice(&iv);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[start..4032]);
        mac.update(&((index + 1) as u32).to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        encrypted.extend_from_slice(&page);
    }
    fs::write(output, encrypted).unwrap();
}

#[track_caller]
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

// 记录整个合成输出树，拒绝操作不得偷偷认领、改写元数据或留下暂存文件。
fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "合成输出中不应有符号链接");
            let value = if kind.is_dir() {
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            out.insert(path.strip_prefix(root).unwrap().to_path_buf(), value);
            if kind.is_dir() {
                visit(root, &path, out);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[track_caller]
fn rejected(output: Output, reason: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "拒绝操作意外成功: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains(reason),
        "拒绝原因不符: status={}\nstdout={}\nstderr={stderr}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
}

#[track_caller]
fn counts(summary: &Value, expected: &[(&str, u64)]) {
    for name in COUNTERS {
        let value = expected
            .iter()
            .find(|(key, _)| key == name)
            .map_or(0, |(_, value)| *value);
        assert_eq!(summary[*name], json!(value), "计数字段 {name}");
    }
}

fn media(kind: u8, url: &str) -> String {
    format!("<media><id>synthetic-media-{kind}</id><type>{kind}</type><url>{url}</url></media>")
}

// 只监听本机临时端口；即使被测进程失败，退出也会停止并回收线程。
struct HttpFixture {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HttpFixture {
    fn new() -> Self {
        Self::body(MP4.to_vec())
    }

    fn body(payload: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/synthetic.mp4", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let count = requests.clone();
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        count.fetch_add(1, Ordering::SeqCst);
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let _ = stream.read(&mut request);
                        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
                        let _ = stream.write_all(header.as_bytes());
                        // 分段发送，不能依赖一次网络读取恰好填满解密前缀。
                        for chunk in payload.chunks(4093) {
                            if stream.write_all(chunk).is_err() {
                                break;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("本地监听失败: {error}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn encrypted_feed_fixture_works_without_python() {
    let mut f = Fixture::new();
    let profile = f.account(
        "feed",
        &[(10, 1704196800, USER, "fixture-ready", String::new())],
    );
    let feed = success(f.run(&profile, &["sns-feed", "--user", USER, "--json"]));
    assert_eq!(feed[0]["content"], "fixture-ready");
    assert_eq!(feed.as_array().unwrap().len(), 1);
    f.unchanged();
}

#[test]
fn no_remote_preserves_post_list_and_filters_html_and_dates() {
    let server = HttpFixture::new();
    let mut f = Fixture::new();
    let profile = f.account(
        "offline",
        &[
            (11, 1704110400, USER, "older-text", String::new()),
            (
                12,
                1704196800,
                USER,
                "safe &lt;script&gt;alert(1)&lt;/script&gt;",
                String::new(),
            ),
            (13, 1704283200, USER, "", media(2, &server.url)),
            (14, 1704369600, USER, "", media(6, &server.url)),
            (15, 1704456000, USER, "", media(15, &server.url)),
            (16, 1704542400, USER, "   ", media(1, &server.url)),
            (
                17,
                1704628800,
                "synthetic-other-author",
                "wrong-author",
                String::new(),
            ),
        ],
    );
    let out = f.root.path().join("offline-album");
    let summary = f.album(&profile, &out, &["--no-remote"]);
    counts(
        &summary,
        &[
            ("posts", 6),
            ("album_posts", 2),
            ("image_missing", 1),
            ("video_total", 2),
            ("video_missing", 2),
        ],
    );
    let posts = read_json(&out.join("timeline.json"));
    let posts = posts.as_array().expect("timeline 必须是帖子数组");
    assert_eq!(posts.len(), 6);
    assert_eq!(summary["first"], posts.last().unwrap()["time"]);
    assert_eq!(summary["last"], posts[0]["time"]);
    assert!(posts.iter().all(|p| p["author_username"] == USER));
    let html = fs::read_to_string(out.join("timeline.html")).unwrap();
    assert_eq!(html.matches("<article").count(), 2);
    assert!(html.contains("older-text"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("wrong-author"));
    assert!(!html.contains("<img "));
    assert!(!html.contains("<video "));
    let filtered = f.root.path().join("filtered");
    let summary = f.album(
        &profile,
        &filtered,
        &[
            "--no-remote",
            "--since",
            "2024-01-02",
            "--until",
            "2024-01-03",
            "-n",
            "1",
        ],
    );
    counts(&summary, &[("posts", 1), ("image_missing", 1)]);
    assert_eq!(read_json(&filtered.join("timeline.json"))[0]["tid"], 13);
    assert_eq!(server.count(), 0, "--no-remote 不应发起连接");
}

#[test]
fn existing_images_and_both_video_types_are_reused_without_network() {
    let server = HttpFixture::new();
    let mut f = Fixture::new();
    let profile = f.account(
        "reuse",
        &[(
            30,
            1704196800,
            USER,
            "",
            format!(
                "{}{}{}",
                media(2, &server.url),
                media(6, &server.url),
                media(15, &server.url)
            ),
        )],
    );
    let out = f.root.path().join("existing");
    fs::create_dir_all(out.join("images")).unwrap();
    fs::create_dir_all(out.join("videos")).unwrap();
    let image = out.join("images/00001_30_01.png");
    let video = out.join("videos/00001_30_02.mp4");
    let second_video = out.join("videos/00001_30_03.mp4");
    fs::write(&image, PNG).unwrap();
    fs::write(&video, MP4).unwrap();
    fs::write(&second_video, MP4).unwrap();
    let before = snapshot(&out);
    rejected(
        f.run(
            &profile,
            &["sns-album", USER, "--output-dir", out.to_str().unwrap()],
        ),
        "requires Adopt",
    );
    let mut after = snapshot(&out);
    // 发布器保留空锁作为同步锚点；仅允许这一新增文件，旧数据和绑定不许改变。
    if let Some(lock) = after.remove(Path::new(".wx-sns-publish.lock")) {
        assert_eq!(lock, Some(Vec::new()), "同步锁不得包含业务数据");
    }
    assert_eq!(after, before);
    assert_eq!(server.count(), 0);
    f.unchanged();
    let summary = f.album(
        &profile,
        &out,
        &[
            "--adopt-existing",
            "--image-workers",
            "2",
            "--video-workers",
            "2",
        ],
    );
    assert_eq!(summary["legacy_unverified"], true);
    let binding = fs::read(out.join("_source_binding.json")).unwrap();
    counts(
        &summary,
        &[
            ("posts", 1),
            ("album_posts", 1),
            ("image_ok", 1),
            ("image_existing", 1),
            ("video_total", 2),
            ("video_ok", 2),
            ("video_complete", 2),
            ("video_existing", 2),
        ],
    );
    let posts = read_json(&out.join("timeline.json"));
    assert_eq!(posts[0]["media"][0]["image_source"], "existing");
    assert_eq!(posts[0]["media"][0]["local_file"], "images/00001_30_01.png");
    for index in [1, 2] {
        assert_eq!(posts[0]["media"][index]["video_source"], "existing");
        assert_eq!(posts[0]["media"][index]["video_complete"], true);
        assert_eq!(posts[0]["media"][index]["video_bytes"], MP4.len());
    }
    let html = fs::read_to_string(out.join("timeline.html")).unwrap();
    assert_eq!(html.matches("<img ").count(), 1);
    assert_eq!(html.matches("<video ").count(), 2);
    let skipped = f.album(&profile, &out, &["--no-videos"]);
    assert_eq!(
        skipped["legacy_unverified"], true,
        "自动更新不能洗掉旧媒体来源警告"
    );
    assert_eq!(fs::read(out.join("_source_binding.json")).unwrap(), binding);
    counts(
        &skipped,
        &[
            ("posts", 1),
            ("album_posts", 1),
            ("image_ok", 1),
            ("image_existing", 1),
            ("video_total", 2),
            ("video_skipped", 2),
        ],
    );
    assert!(!fs::read_to_string(out.join("timeline.html"))
        .unwrap()
        .contains("<video "));
    assert_eq!(fs::read(image).unwrap(), PNG);
    assert_eq!(fs::read(video).unwrap(), MP4);
    assert_eq!(fs::read(second_video).unwrap(), MP4);
    assert_eq!(server.count(), 0);
}

#[test]
fn loopback_plain_video_is_downloaded_and_counted() {
    let server = HttpFixture::new();
    let mut f = Fixture::new();
    let profile = f.account(
        "download",
        &[(40, 1704196800, USER, "", media(6, &server.url))],
    );
    let out = f.root.path().join("downloaded");
    let summary = f.album(&profile, &out, &[]);
    counts(
        &summary,
        &[
            ("posts", 1),
            ("album_posts", 1),
            ("video_total", 1),
            ("video_ok", 1),
            ("video_complete", 1),
            ("video_remote", 1),
        ],
    );
    assert_eq!(fs::read(out.join("videos/00001_40_01.mp4")).unwrap(), MP4);
    assert_eq!(server.count(), 1);
}

#[test]
fn loopback_oracle_encrypted_video_decrypts_prefix_and_preserves_tail() {
    const PREFIX: usize = 128 * 1024;
    // 复用原 Node/WASM 包装器生成并由 video_runtime 测试核验的公开合成向量。
    // 本测试不启动 Node，也不调用被测 Rust 解密器反向生成 oracle。
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/sns-video-native/vectors.json")).unwrap();
    let vector = vectors
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["key"] == "1" && v["size"] == PREFIX)
        .expect("缺少完整 128 KiB 合成 oracle");
    let hex = vector["hex"].as_str().unwrap();
    assert_eq!(hex.len(), PREFIX * 2);
    let mask: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let mut expected: Vec<u8> = (0..PREFIX + 32779)
        .map(|i| ((i * 37 + 19) % 251) as u8)
        .collect();
    expected[..MP4.len()].copy_from_slice(MP4);
    let mut encrypted = expected.clone();
    for (byte, key) in encrypted[..PREFIX].iter_mut().zip(mask) {
        *byte ^= key;
    }
    assert_ne!(&encrypted[..PREFIX], &expected[..PREFIX]);
    assert_ne!(&encrypted[4..8], b"ftyp", "必须真正走加密分支");
    assert_eq!(&encrypted[PREFIX..], &expected[PREFIX..]);
    let server = HttpFixture::body(encrypted.clone());
    let mut f = Fixture::new();
    let xml = format!(
        "<media><id>oracle-encrypted-video</id><type>6</type><url>{}</url><enc key=\"1\"/></media>",
        server.url
    );
    let profile = f.account("encrypted-video", &[(100, 1704196800, USER, "", xml)]);
    let out = f.root.path().join("encrypted-downloaded");
    let summary = f.album(&profile, &out, &[]);
    counts(
        &summary,
        &[
            ("posts", 1),
            ("album_posts", 1),
            ("video_total", 1),
            ("video_ok", 1),
            ("video_complete", 1),
            ("video_remote", 1),
        ],
    );
    assert_eq!(summary["engine"], "rust");
    let posts = read_json(&out.join("timeline.json"));
    assert_eq!(posts.as_array().unwrap().len(), 1);
    let media = &posts[0]["media"][0];
    assert_eq!(media["enc_key"], "1", "Feed 必须把 XML 密钥传给下载链路");
    assert_eq!(media["local_file"], "videos/00001_100_01.mp4");
    assert_eq!(media["video_source"], "remote");
    assert_eq!(media["video_complete"], true);
    assert_eq!(media["video_bytes"], expected.len());
    assert!(media.get("video_error").is_none());
    let actual = fs::read(out.join(media["local_file"].as_str().unwrap())).unwrap();
    assert_eq!(&actual[..PREFIX], &expected[..PREFIX]);
    assert_eq!(
        &actual[PREFIX..],
        &encrypted[PREFIX..],
        "尾部必须保持 HTTP 原字节"
    );
    assert_eq!(actual, expected);
    assert!(fs::read_to_string(out.join("timeline.html"))
        .unwrap()
        .contains("videos/00001_100_01.mp4"));
    assert_eq!(server.count(), 1);
    f.unchanged();
}

#[test]
fn complete_and_partial_cache_keep_mtime_in_final_album() {
    let server = HttpFixture::new();
    let mut f = Fixture::new();
    for (index, complete) in [true, false].into_iter().enumerate() {
        let tid = 110 + index as i64;
        let profile = f.account(
            &format!("cache-mtime-{index}"),
            &[(tid, 1704196800, USER, "", media(6, &server.url))],
        );
        // 沿用真实缓存规则 md5(post_id + '_' + media_id + '_3')，分片目录占前两位。
        let key = format!("{:x}", md5::compute(format!("{tid}_synthetic-media-6_3")));
        let source = profile
            .join("cache/2024-01/Sns/Video")
            .join(&key[..2])
            .join(format!(
                "{}.{}",
                &key[2..],
                if complete { "mp4" } else { "temp" }
            ));
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, MP4).unwrap();
        let modified = std::time::UNIX_EPOCH + Duration::from_secs(1600000000 + index as u64);
        fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let source_time = fs::metadata(&source).unwrap().modified().unwrap();
        let out = f.root.path().join(format!("cache-mtime-output-{index}"));
        let summary = f.album(&profile, &out, &["--no-remote"]);
        counts(
            &summary,
            &[
                ("posts", 1),
                ("album_posts", 1),
                ("video_total", 1),
                ("video_ok", 1),
                ("video_complete", u64::from(complete)),
                ("video_cache", u64::from(complete)),
                ("video_partial_cache", u64::from(!complete)),
            ],
        );
        let posts = read_json(&out.join("timeline.json"));
        let item = &posts[0]["media"][0];
        assert_eq!(item["video_complete"], complete);
        let target = out.join(item["local_file"].as_str().unwrap());
        assert_eq!(fs::read(&target).unwrap(), MP4);
        assert_eq!(
            fs::metadata(&target).unwrap().modified().unwrap(),
            source_time,
            "缓存 mtime 必须穿过 staging/publisher 保留在最终文件"
        );
        assert_eq!(fs::read(&source).unwrap(), MP4);
        assert_eq!(
            fs::metadata(&source).unwrap().modified().unwrap(),
            source_time
        );
        f.unchanged();
    }
    assert_eq!(server.count(), 0);
}

#[test]
fn empty_timeline_and_output_root_contract() {
    let mut f = Fixture::new();
    let profile = f.account("empty", &[]);
    let root = f.root.path().join("albums");
    let summary = success(f.run(
        &profile,
        &[
            "sns-album",
            USER,
            "--output-root",
            root.to_str().unwrap(),
            "--no-remote",
        ],
    ));
    counts(&summary, &[]);
    assert!(summary["first"].is_null());
    assert!(summary["last"].is_null());
    let out = Path::new(summary["output_dir"].as_str().unwrap());
    assert_eq!(
        fs::canonicalize(out.parent().unwrap()).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    assert_eq!(read_json(&out.join("timeline.json")), json!([]));
    assert_eq!(read_json(&out.join("export_summary.json")), summary);
    assert!(!fs::read_to_string(out.join("timeline.html"))
        .unwrap()
        .contains("<article"));
    f.unchanged();
}

#[test]
fn invalid_arguments_do_not_create_output() {
    let mut f = Fixture::new();
    let profile = f.account("invalid", &[]);
    // 先证明有效入口可用，避免未知参数导致所有负例意外通过。
    f.album(
        &profile,
        &f.root.path().join("valid-control"),
        &["--no-remote"],
    );
    for (index, extra) in [
        vec!["--since", "not-a-date"],
        vec!["--since", "2024-02-02", "--until", "2024-01-01"],
        vec!["-n", "not-a-number"],
        vec!["--image-workers", "bad"],
        vec!["--video-workers", "bad"],
        vec!["--unknown-option"],
        vec!["--output", "conflicting-output"],
    ]
    .iter()
    .enumerate()
    {
        let out = f.root.path().join(format!("invalid-{index}"));
        let mut args = vec!["sns-album", USER, "--output-dir", out.to_str().unwrap()];
        args.extend_from_slice(extra);
        let result = f.run(&profile, &args);
        assert!(!result.status.success(), "无效参数被接受: {args:?}");
        assert!(!result.stderr.is_empty());
        assert!(!out.exists(), "参数验证前创建了输出目录");
        assert!(!f.root.path().join("conflicting-output").exists());
    }
    f.unchanged();
}

#[test]
fn zero_limit_and_zero_workers_preserve_legacy_clamping() {
    let mut f = Fixture::new();
    let profile = f.account(
        "zero",
        &[(70, 1704196800, USER, "zero-workers-text", String::new())],
    );
    let out = f.root.path().join("zero-limit");
    let summary = f.album(
        &profile,
        &out,
        &[
            "-n",
            "0",
            "--image-workers",
            "0",
            "--video-workers",
            "0",
            "--no-remote",
        ],
    );
    counts(&summary, &[]);
    assert_eq!(read_json(&out.join("timeline.json")), json!([]));
    let out = f.root.path().join("zero-workers");
    let summary = f.album(
        &profile,
        &out,
        &[
            "--image-workers",
            "0",
            "--video-workers",
            "0",
            "--no-remote",
        ],
    );
    counts(&summary, &[("posts", 1), ("album_posts", 1)]);
}

#[test]
fn shared_runtime_keeps_accounts_and_sources_isolated() {
    let mut f = Fixture::new();
    let a = f.account(
        "account-a",
        &[(50, 1704196800, USER, "account-a-only", String::new())],
    );
    let b = f.account(
        "account-b",
        &[(60, 1704196800, USER, "account-b-only", String::new())],
    );
    for (index, (profile, expected, forbidden)) in [
        (&a, "account-a-only", "account-b-only"),
        (&b, "account-b-only", "account-a-only"),
        (&a, "account-a-only", "account-b-only"),
    ]
    .iter()
    .enumerate()
    {
        let out = f.root.path().join(format!("isolated-{index}"));
        let summary = f.album(profile, &out, &["--no-remote"]);
        counts(&summary, &[("posts", 1), ("album_posts", 1)]);
        let posts = read_json(&out.join("timeline.json"));
        assert_eq!(posts[0]["content"], *expected);
        assert!(!posts.to_string().contains(forbidden));
    }
    f.unchanged();
}

#[test]
fn empty_output_dir_binds_then_updates_without_adoption() {
    let mut f = Fixture::new();
    let profile = f.account(
        "new-binding",
        &[(80, 1704196800, USER, "bound-text", String::new())],
    );
    let out = f.root.path().join("empty-existing-directory");
    fs::create_dir(&out).unwrap();
    let first = f.album(&profile, &out, &["--no-remote"]);
    assert_eq!(first["legacy_unverified"], false);
    counts(&first, &[("posts", 1), ("album_posts", 1)]);
    let binding = fs::read(out.join("_source_binding.json")).unwrap();
    assert_eq!(
        read_json(&out.join("_source_binding.json"))["user_name"],
        USER
    );
    // 改变查询范围，证明是自动更新而不只是返回旧摘要。
    let second = f.album(&profile, &out, &["--no-remote", "-n", "0"]);
    counts(&second, &[]);
    assert_eq!(second["legacy_unverified"], false);
    assert_eq!(read_json(&out.join("timeline.json")), json!([]));
    assert_eq!(fs::read(out.join("_source_binding.json")).unwrap(), binding);
}

#[test]
fn adoption_cannot_override_another_account_or_contact_binding() {
    let mut f = Fixture::new();
    let a = f.account(
        "binding-account-a",
        &[
            (90, 1704196800, USER, "owner-a", String::new()),
            (
                91,
                1704196800,
                "synthetic-other-author",
                "other-contact",
                String::new(),
            ),
        ],
    );
    let b = f.account(
        "binding-account-b",
        &[(90, 1704196800, USER, "owner-b", String::new())],
    );
    let out = f.root.path().join("protected-binding");
    let first = f.album(&a, &out, &["--no-remote"]);
    assert_eq!(first["legacy_unverified"], false);
    // 两个账号拥有相同联系人名字；拒绝必须基于账号来源，而非仅看联系人。
    let before = snapshot(&out);
    for (profile, user) in [(&b, USER), (&a, "synthetic-other-author")] {
        for adopt in [false, true] {
            let mut args = vec![
                "sns-album",
                user,
                "--output-dir",
                out.to_str().unwrap(),
                "--no-remote",
            ];
            if adopt {
                args.push("--adopt-existing");
            }
            rejected(f.run(profile, &args), "source binding conflict");
            assert_eq!(snapshot(&out), before, "拒绝后绑定或输出被修改");
            f.unchanged();
        }
    }
    let updated = f.album(&a, &out, &["--no-remote"]);
    counts(&updated, &[("posts", 1), ("album_posts", 1)]);
    assert_eq!(
        read_json(&out.join("timeline.json"))[0]["content"],
        "owner-a"
    );
}
