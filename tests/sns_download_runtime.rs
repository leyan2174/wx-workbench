//! 原生 SNS 下载 CLI 契约：仅 loopback 和合成数据库，不验证旧 alias 迁移。
use base64::Engine;
#[path = "support/bootstrap.rs"]
mod bootstrap;
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<String>>>,
}

impl Server {
    fn new(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            while !done.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("loopback accept: {error}"),
                };
                // Windows 接受的 socket 可能继承非阻塞模式，读请求前显式恢复。
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                drop(reader);
                requests.push(path.clone());
                let (_, status, body) = routes
                    .iter()
                    .find(|(route, _, _)| *route == path)
                    .unwrap_or_else(|| panic!("unexpected loopback request: {path}"));
                let reason = if *status == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                // 故意给出不可信 MIME；扩展名应由实际响应字节决定。
                write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body).unwrap();
            }
            requests
        });
        Self {
            address,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
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

struct Fixture {
    root: tempfile::TempDir,
    original_db: Vec<u8>,
}

impl Fixture {
    fn new(urls: &[String]) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("ambient.json"), b"invalid ambient config").unwrap();
        fs::create_dir(root.path().join("source")).unwrap();
        let path = root.path().join("source/sns.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT); CREATE TABLE SnsMessage_tmp3(feed_id INTEGER,create_time INTEGER,type INTEGER,from_username TEXT,from_nickname TEXT,to_username TEXT,to_nickname TEXT,content TEXT,del_status INTEGER);").unwrap();
        let media = urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                format!("<media><id>media{i}</id><type>2</type><url>{url}</url></media>")
            })
            .collect::<String>();
        for (id, text, media) in [
            (1, "synthetic media post", media.as_str()),
            (2, "synthetic text post", ""),
        ] {
            let xml = format!("<root><TimelineObject><id>{id}</id><username>synthetic</username><createTime>{}</createTime><contentDesc>{text}</contentDesc><ContentObject><contentStyle>1</contentStyle><mediaList>{media}</mediaList></ContentObject></TimelineObject></root>", 1710000000 - id);
            db.execute(
                "INSERT INTO SnsTimeLine VALUES (?1,'synthetic',?2)",
                rusqlite::params![id, xml],
            )
            .unwrap();
        }
        drop(db);
        let original_db = fs::read(path).unwrap();
        Self { root, original_db }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn run(&self, output: &str, options: &[&str], ambient_download: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(["toolkit", "export-sns-native"])
            .arg(self.path("source/sns.db"))
            .arg(self.path(output))
            .args(options)
            .current_dir(self.root.path())
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_CLI_EXPECTED_RUNTIME")
            .env_remove("WECHAT_EXPORT_CONTACTS")
            .env_remove("WECHAT_EXPORT_USERS")
            .env_remove("WECHAT_SNS_DOWNLOAD_MEDIA")
            .env("WX_CLI_CONFIG", self.path("ambient.json"))
            .env("WX_CLI_HOME", self.path("runtime"))
            .env("PATH", "")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
        if let Some(value) = ambient_download {
            command.env("WECHAT_SNS_DOWNLOAD_MEDIA", value);
        }
        let result = command.output().unwrap();
        assert_eq!(
            fs::read(self.path("ambient.json")).unwrap(),
            b"invalid ambient config"
        );
        bootstrap::assert_only_bootstrap(&self.path("runtime"));
        assert_eq!(
            fs::read(self.path("source/sns.db")).unwrap(),
            self.original_db
        );
        assert_eq!(fs::read_dir(self.path("source")).unwrap().count(), 1);
        result
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(bootstrap::BootstrapCleanup(self.path("runtime")));
    }
}

fn diagnostic(output: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn audit_for(root: &Path, post: &Value) -> Value {
    // audit 按发布文件顺序排列，不能假定与 timeline 的倒序时间排列一致。
    json(&root.join("_media_recovery.json"))
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| json(&root.join(entry["post_file"].as_str().unwrap()))["id"] == post["id"])
        .unwrap()
        .clone()
}

fn png() -> Vec<u8> {
    let mut bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aC1sAAAAASUVORK5CYII=").unwrap();
    // 满足下载器 100 字节下限；这里只验证格式识别与原样发布，不验证解码。
    bytes.resize(128, 0);
    bytes
}

#[test]
fn default_and_ambient_env_do_not_authorize_http() {
    let server = Server::new(vec![("/image.jpg", 200, png())]);
    let f = Fixture::new(&[server.url("/image.jpg")]);
    for (out, env) in [("default", None), ("ambient", Some("1"))] {
        let result = f.run(out, &[], env);
        assert!(result.status.success(), "{}", diagnostic(&result));
        let root = f.path(out).join("synthetic/SNS");
        let timeline = json(&root.join("timeline.json"));
        assert_eq!(timeline["total_posts"], 2);
        assert!(timeline["posts"][0]["media"][0].get("local_file").is_none());
        let html = fs::read_to_string(root.join("timeline.html")).unwrap();
        assert!(!html.contains("src=\"http"));
        assert!(html.contains("synthetic media post"));
    }
    assert!(
        server.finish().is_empty(),
        "无 flag 不得发出 HTTP 请求，包括 env=1"
    );
}

#[test]
fn explicit_flag_downloads_actual_extensions_and_consistent_json_html() {
    let png = png();
    let mut gif = base64::engine::general_purpose::STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
        .unwrap();
    gif.resize(128, 0);
    let server = Server::new(vec![
        ("/not-png.jpg", 200, png.clone()),
        ("/not-gif.png", 200, gif.clone()),
    ]);
    let f = Fixture::new(&[server.url("/not-png.jpg"), server.url("/not-gif.png")]);
    let result = f.run("out", &["--download-media"], None);
    assert!(result.status.success(), "{}", diagnostic(&result));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["media_downloaded"], 2);
    assert_eq!(report["media_download_failed"], 0);
    let root = f.path("out/synthetic/SNS");
    let timeline = json(&root.join("timeline.json"));
    let post = &timeline["posts"][0];
    let audit = audit_for(&root, post);
    assert_eq!(
        *post,
        json(&root.join(audit["post_file"].as_str().unwrap()))
    );
    let html = fs::read_to_string(root.join("timeline.html")).unwrap();
    for (i, (extension, bytes)) in [("png", png), ("gif", gif)].into_iter().enumerate() {
        let name = post["media"][i]["local_file"].as_str().unwrap();
        assert_eq!(Path::new(name).extension().unwrap(), extension);
        assert_eq!(post["media"][i]["download_format"], extension);
        assert_eq!(fs::read(root.join(name)).unwrap(), bytes);
        assert_eq!(
            audit["recovery"]["media"][i]["reference"]["local_file"],
            name
        );
        assert!(html.contains(&format!("<img src=\"{name}\"")));
    }
    assert!(!html.contains("src=\"http"));
    assert_eq!(server.finish(), ["/not-png.jpg", "/not-gif.png"]);
}

#[test]
fn http_failure_is_visible_and_keeps_posts_and_other_media() {
    let server = Server::new(vec![
        ("/bad", 503, b"synthetic failure".to_vec()),
        ("/good", 200, png()),
    ]);
    let f = Fixture::new(&[server.url("/bad"), server.url("/good")]);
    let result = f.run("out", &["--download-media"], None);
    assert!(!result.status.success(), "{}", diagnostic(&result));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap_or_else(|error| {
        panic!("missing failure summary: {error}; {}", diagnostic(&result))
    });
    assert_eq!(report["posts"], 2);
    assert_eq!(report["media_downloaded"], 1);
    assert_eq!(report["media_download_failed"], 1);
    assert_eq!(report["media_failed"], 1);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    let root = f.path("out/synthetic/SNS");
    let timeline = json(&root.join("timeline.json"));
    assert_eq!(timeline["total_posts"], 2);
    assert_eq!(timeline["posts"][0]["content_desc"], "synthetic media post");
    assert_eq!(timeline["posts"][1]["content_desc"], "synthetic text post");
    let media = &timeline["posts"][0]["media"];
    assert!(media[0].get("local_file").is_none());
    assert_eq!(
        fs::read(root.join(media[1]["local_file"].as_str().unwrap())).unwrap(),
        png()
    );
    let audit = audit_for(&root, &timeline["posts"][0]);
    assert_eq!(audit["recovery"]["media"][0]["status"], "download_failed");
    let html = fs::read_to_string(root.join("timeline.html")).unwrap();
    assert!(html.contains("synthetic media post") && html.contains("synthetic text post"));
    assert_eq!(server.finish(), ["/bad", "/good"]);
}

#[test]
fn help_lists_download_flag_before_config_loading_without_python() {
    let f = Fixture::new(&[]);
    let result = f.run("help-out", &["--help"], Some("1"));
    assert!(result.status.success(), "{}", diagnostic(&result));
    assert!(String::from_utf8_lossy(&result.stdout).contains("--download-media"));
    assert!(!f.path("help-out").exists());
}
