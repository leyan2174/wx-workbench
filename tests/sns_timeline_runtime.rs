//! 真实 CLI 的静态合成 SQLite 契约；daemon 执行业务，不依赖 Python。
#[path = "support/bootstrap.rs"]
mod bootstrap;
#[path = "support/key_store.rs"]
mod key_store_fixture;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const ALICE: &str = "synthetic-alice";
const BOB: &str = "synthetic-bob";
const STAMP: i64 = 1710000000;

struct Fixture {
    root: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(bootstrap::RuntimeCleanup(self.root.path().join("runtime")));
    }
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("wx-sns-timeline-")
            .tempdir()
            .unwrap();
        fs::create_dir(root.path().join("runtime")).unwrap();
        // 工作目录及旧工具目录放置坏配置，证明只使用明确选中的账号。
        fs::write(root.path().join("config.json"), b"invalid ambient config").unwrap();
        Self { root }
    }

    fn account(&self, name: &str) -> PathBuf {
        let p = self.root.path().join(name);
        for dir in ["decrypted/sns", "decrypted/contact", "db_storage", "cache"] {
            fs::create_dir_all(p.join(dir)).unwrap();
        }
        let db = Connection::open(p.join("decrypted/sns/sns.db")).unwrap();
        db.execute_batch(include_str!("fixtures/sns-timeline-runtime/schema.sql"))
            .unwrap();
        let contacts = Connection::open(p.join("decrypted/contact/contact.db")).unwrap();
        contacts.execute_batch("CREATE TABLE contact(username TEXT, nick_name TEXT, remark TEXT, verify_flag INTEGER);").unwrap();
        for (user, display) in [(ALICE, "Alice"), (BOB, "Bob")] {
            contacts
                .execute(
                    "INSERT INTO contact VALUES (?1,?2,'',0)",
                    params![user, display],
                )
                .unwrap();
        }
        fs::write(
            p.join("config.json"),
            json!({
                "db_dir":"db_storage", "keys_file":"all_keys.json",
                "decrypted_dir":"decrypted", "output_base_dir":"exports",
                "image_xor_key":136
            })
            .to_string(),
        )
        .unwrap();
        key_store_fixture::migrate_with_unverified(
            Path::new(env!("CARGO_BIN_EXE_wx")),
            &p.join("config.json"),
            &self.root.path().join("runtime"),
            true,
        );
        p
    }

    fn post(&self, account: &Path, id: i64, user: &str, text: &str, media: &str) {
        let db = Connection::open(account.join("decrypted/sns/sns.db")).unwrap();
        // 与真实导出解析器及 export_download_tests 一致：TimelineObject 是容器的后代。
        let xml = format!("<root><TimelineObject><id>{id}</id><username>{user}</username><createTime>{STAMP}</createTime><contentDesc>{text}</contentDesc><ContentObject><type>1</type><mediaList>{media}</mediaList></ContentObject></TimelineObject></root>");
        db.execute("DELETE FROM SnsTimeLine WHERE tid=?1", [id])
            .unwrap();
        db.execute(
            "INSERT INTO SnsTimeLine VALUES (?1,?2,?3)",
            params![id, user, xml],
        )
        .unwrap();
    }

    fn run(&self, account: &Path, args: &[&str]) -> Output {
        self.run_env(account, args, &[])
    }

    fn run_env(&self, account: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let source = tree(&account.join("decrypted"));
        let config = fs::read(account.join("config.json")).unwrap();
        let cache = tree(&account.join("cache"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(args)
            .current_dir(self.root.path())
            .env("WX_CLI_CONFIG", account.join("config.json"))
            .env("WX_CLI_HOME", self.root.path().join("runtime"))
            .env(
                "WX_WECHAT_DECRYPT_DIR",
                self.root.path().join("missing-toolkit"),
            )
            .env(
                "WX_WECHAT_DECRYPT_PYTHON",
                self.root.path().join("missing-python.exe"),
            )
            .env("PATH", "")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
        for key in [
            "WX_DAEMON_MODE",
            "WX_CLI_EXPECTED_RUNTIME",
            "WECHAT_EXPORT_CONTACTS",
            "WECHAT_EXPORT_USERS",
            "WECHAT_SNS_DOWNLOAD_MEDIA",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env_remove(key);
        }
        // 只设置当前子进程环境，避免并行测试间污染进程全局状态。
        command.envs(env.iter().copied());
        // 输出完整命令及两个流，失败时无需重新构造现场。
        eprintln!("command={command:?}");
        let result = command.output().unwrap();
        eprintln!("{}", diagnostic(&result));
        assert_eq!(tree(&account.join("decrypted")), source, "源数据库树被修改");
        assert_eq!(tree(&account.join("cache")), cache, "缓存源被修改");
        assert_eq!(fs::read(account.join("config.json")).unwrap(), config);
        let accounts = self.root.path().join("runtime/accounts");
        if accounts.exists() {
            for runtime in fs::read_dir(accounts).unwrap() {
                assert_eq!(
                    fs::read_dir(runtime.unwrap().path().join("cache"))
                        .unwrap()
                        .count(),
                    0,
                    "static snapshot loaded account query databases"
                );
            }
        }
        result
    }

    fn alias(&self, account: &Path, out: Option<&Path>, contacts: &str, extra: &[&str]) -> Output {
        let mut args = vec!["toolkit", "export-sns", "--contacts", contacts];
        if let Some(out) = out {
            args.extend(["--output-dir", out.to_str().unwrap()]);
        }
        args.extend_from_slice(extra);
        self.run(account, &args)
    }

    fn native(&self, account: &Path, out: &Path, extra: &[&str]) -> Output {
        let sns = account.join("decrypted/sns/sns.db");
        let contacts = account.join("decrypted/contact/contact.db");
        let mut args = vec![
            "toolkit",
            "export-sns-native",
            sns.to_str().unwrap(),
            out.to_str().unwrap(),
            "--contact-db",
            contacts.to_str().unwrap(),
            "--contacts",
            ALICE,
        ];
        args.extend_from_slice(extra);
        self.run(account, &args)
    }

    fn runtime_id(&self, account: &Path) -> String {
        // 独立按 RuntimeContext v2 的公开路径契约计算，不依赖内部模块。
        let mut digest = Sha256::new();
        digest.update(b"wx-cli-runtime-v2\0");
        for path in [
            account.join("config.json"),
            account.join("db_storage"),
            account.join("all_keys.json"),
            self.root.path().join("runtime"),
        ] {
            digest.update(
                path.canonicalize()
                    .unwrap_or_else(|_| {
                        path.parent()
                            .unwrap()
                            .canonicalize()
                            .unwrap()
                            .join(path.file_name().unwrap())
                    })
                    .to_string_lossy()
                    .to_lowercase()
                    .as_bytes(),
            );
            digest.update([0]);
        }
        format!("{:x}", digest.finalize())
    }
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, dir: &Path, map: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            assert!(!entry.file_type().unwrap().is_symlink());
            let value = if path.is_dir() {
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            map.insert(path.strip_prefix(root).unwrap().to_path_buf(), value);
            if path.is_dir() {
                visit(root, &path, map);
            }
        }
    }
    let mut map = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut map);
    }
    map
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn diagnostic(o: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}
#[track_caller]
fn success(o: Output) -> Value {
    assert!(o.status.success(), "{}", diagnostic(&o));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["engine"], "rust");
    v
}
#[track_caller]
fn rejected(o: Output) {
    assert!(!o.status.success(), "{}", diagnostic(&o));
    assert!(!o.stderr.is_empty(), "拒绝必须提供诊断");
}

// 仅允许发布器增加空锁；既有文件、目录及绑定逐字节不变。
fn preserved(root: &Path, before: &BTreeMap<PathBuf, Option<Vec<u8>>>) {
    let mut after = tree(root);
    let new_locks: Vec<_> = after
        .keys()
        .filter(|p| {
            !before.contains_key(*p) && p.file_name().is_some_and(|s| s == ".wx-sns-publish.lock")
        })
        .cloned()
        .collect();
    for p in new_locks {
        assert_eq!(after.remove(&p), Some(Some(Vec::new())));
    }
    assert_eq!(&after, before);
}

fn consistent(dir: &Path, expected: usize) -> Value {
    let timeline = read_json(&dir.join("timeline.json"));
    let posts = timeline["posts"].as_array().unwrap();
    assert_eq!(posts.len(), expected);
    assert_eq!(timeline["total_posts"], expected);
    let html = fs::read_to_string(dir.join("timeline.html")).unwrap();
    let singles: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "json"))
        .map(|p| read_json(&p))
        .filter(|v| v.get("id").is_some())
        .collect();
    for post in posts {
        assert_eq!(
            singles.iter().filter(|s| *s == post).count(),
            1,
            "单帖与汇总必须完全一致且无同秒覆盖"
        );
        assert!(html.contains(post["content_desc"].as_str().unwrap()));
        for media in post["media"].as_array().unwrap() {
            if let Some(file) = media["local_file"].as_str() {
                assert!(dir.join(file).is_file());
                assert!(html.contains(file));
            }
        }
    }
    timeline
}

#[test]
fn selected_config_collision_filter_and_automatic_update() {
    let f = Fixture::new();
    let a = f.account("a");
    let b = f.account("b");
    for (id, user, text) in [
        (1, ALICE, "first-alice"),
        (2, ALICE, "second-alice"),
        (3, BOB, "only-bob"),
    ] {
        f.post(&a, id, user, text, "");
    }
    f.post(&b, 1, ALICE, "account-b-only", "");
    let report = success(f.alias(&a, None, &format!("{ALICE},{BOB}"), &["--no-remote"]));
    assert_eq!(report["contacts"], 2);
    assert_eq!(report["posts"], 3);
    // legacy 无条件派生输出根；raw output_base_dir 是伪路径，不得被采用。
    let out = a.join("wechat_files/a");
    assert!(!a.join("exports").exists());
    let alice = out.join("Alice/SNS");
    consistent(&alice, 2);
    consistent(&out.join("Bob/SNS"), 1);
    let binding = read_json(&alice.join("_source_binding.json"));
    assert_eq!(binding["source_kind"], "account");
    assert_eq!(binding["source_id"], f.runtime_id(&a));
    assert_eq!(binding["user_name"], ALICE);
    fs::write(alice.join("unrelated-old.bin"), b"keep-me").unwrap();
    let bob = tree(&out.join("Bob"));
    f.post(&a, 1, ALICE, "changed-alice", "");
    let report = success(f.alias(&a, None, ALICE, &["--no-remote"]));
    assert_eq!(report["posts"], 2);
    let timeline = consistent(&alice, 2);
    assert!(timeline.to_string().contains("changed-alice"));
    assert!(!timeline.to_string().contains("first-alice"));
    assert!(!timeline.to_string().contains("account-b-only"));
    assert_eq!(read_json(&alice.join("_source_binding.json")), binding);
    assert_eq!(
        fs::read(alice.join("unrelated-old.bin")).unwrap(),
        b"keep-me"
    );
    assert_eq!(tree(&out.join("Bob")), bob);
    success(f.alias(&b, None, ALICE, &["--no-remote"]));
    assert!(consistent(&b.join("wechat_files/b/Alice/SNS"), 1)
        .to_string()
        .contains("account-b-only"));
    assert_ne!(
        read_json(&b.join("wechat_files/b/Alice/SNS/_source_binding.json"))["source_id"],
        binding["source_id"]
    );
    assert!(!b.join("exports").exists());
    let default_before = tree(&out);
    let explicit = f.root.path().join("explicit-output");
    success(f.alias(&a, Some(&explicit), ALICE, &["--no-remote"]));
    consistent(&explicit.join("Alice/SNS"), 2);
    assert_eq!(tree(&out), default_before, "显式输出不能同时更新默认目录");
    assert!(!explicit.join("Bob").exists());
    assert!(!a.join("exports").exists());
}

#[test]
fn legacy_requires_adoption_and_foreign_bindings_are_never_overridden() {
    let f = Fixture::new();
    let a = f.account("a");
    let b = f.account("b");
    for account in [&a, &b] {
        f.post(account, 1, ALICE, "owner", "");
    }
    f.post(&a, 2, BOB, "other-contact", "");
    let out = f.root.path().join("legacy");
    let dir = out.join("Alice/SNS");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("old.json"), b"{\"legacy\":true}").unwrap();
    let before = tree(&out);
    let rejection = f.alias(&a, Some(&out), ALICE, &["--no-remote"]);
    let stderr = String::from_utf8_lossy(&rejection.stderr);
    assert!(
        stderr.contains("unbound nonempty SNS output requires Adopt"),
        "宿主不得隐藏 core 拒绝原因: {stderr}"
    );
    assert!(
        stderr.contains("--adopt-existing"),
        "宿主必须保留认领提示: {stderr}"
    );
    rejected(rejection);
    preserved(&out, &before);
    success(f.alias(&a, Some(&out), ALICE, &["--no-remote", "--adopt-existing"]));
    assert_eq!(
        fs::read(dir.join("old.json")).unwrap(),
        b"{\"legacy\":true}"
    );
    consistent(&dir, 1);
    let before = tree(&out);
    for flags in [vec!["--no-remote"], vec!["--no-remote", "--adopt-existing"]] {
        rejected(f.alias(&b, Some(&out), ALICE, &flags));
        preserved(&out, &before);
    }
    // 让另一个联系人解析到同一展示目录，触发真实联系人绑定冲突。
    let db = Connection::open(a.join("decrypted/contact/contact.db")).unwrap();
    db.execute(
        "UPDATE contact SET nick_name='Alice' WHERE username=?1",
        [BOB],
    )
    .unwrap();
    drop(db);
    rejected(f.alias(&a, Some(&out), BOB, &["--no-remote", "--adopt-existing"]));
    preserved(&out, &before);
}

#[test]
fn empty_results_and_unsafe_outputs_preserve_old_data() {
    let f = Fixture::new();
    let a = f.account("a");
    f.post(&a, 1, ALICE, "old-post", "");
    let out = f.root.path().join("output");
    success(f.alias(&a, Some(&out), ALICE, &["--no-remote"]));
    let before = tree(&out);
    Connection::open(a.join("decrypted/sns/sns.db"))
        .unwrap()
        .execute("DELETE FROM SnsTimeLine", [])
        .unwrap();
    let report = success(f.alias(&a, Some(&out), ALICE, &["--no-remote"]));
    assert_eq!(report["posts"], 0);
    preserved(&out, &before);
    f.post(&a, 1, ALICE, "restored", "");
    for unsafe_out in [
        a.join("decrypted/sns"),
        a.join("decrypted/contact"),
        a.join("decrypted/sns/nested"),
    ] {
        rejected(f.alias(
            &a,
            Some(&unsafe_out),
            ALICE,
            &["--no-remote", "--adopt-existing"],
        ));
    }
    preserved(&out, &before);
}

#[test]
fn late_contact_conflict_preflights_entire_batch_before_any_publish_or_download() {
    let server = Server::new();
    let f = Fixture::new();
    let a = f.account("a");
    let b = f.account("b");
    let contacts = format!("{ALICE},{BOB}");
    // 交换所有权，不依赖 core 按用户名、展示名或查询顺序遍历联系人。
    for (index, owned, foreign) in [(0, ALICE, BOB), (1, BOB, ALICE)] {
        for account in [&a, &b] {
            Connection::open(account.join("decrypted/sns/sns.db"))
                .unwrap()
                .execute("DELETE FROM SnsTimeLine", [])
                .unwrap();
        }
        f.post(&a, 1, owned, "owned-before-update", "");
        f.post(&b, 2, foreign, "foreign-before-update", "");
        let out = f.root.path().join(format!("late-conflict-{index}"));
        success(f.alias(&a, Some(&out), owned, &["--no-remote"]));
        success(f.alias(&b, Some(&out), foreign, &["--no-remote"]));
        for display in ["Alice", "Bob"] {
            fs::write(out.join(display).join("SNS/unrelated.bin"), b"keep").unwrap();
            consistent(&out.join(display).join("SNS"), 1);
        }
        let before = tree(&out);
        let media = format!(
            "<media><id>late-conflict-image</id><type>2</type><url>{}</url></media>",
            server.url
        );
        f.post(&a, 1, owned, "must-not-publish", &media);
        f.post(&a, 2, foreign, "must-not-adopt", "");
        for flags in [
            vec!["--download-media"],
            vec!["--download-media", "--adopt-existing"],
        ] {
            rejected(f.alias(&a, Some(&out), &contacts, &flags));
            preserved(&out, &before);
            assert_eq!(
                server.requests.load(Ordering::SeqCst),
                0,
                "整批来源预检失败，不应先处理其他联系人的媒体"
            );
        }
        // 同样的新内容只选合法联系人能成功，避免负例因输入无效而误通过。
        let report = success(f.alias(&a, Some(&out), owned, &["--no-remote"]));
        assert_eq!(report["posts"], 1);
        let display = if owned == ALICE { "Alice" } else { "Bob" };
        assert!(consistent(&out.join(display).join("SNS"), 1)
            .to_string()
            .contains("must-not-publish"));
    }
}

#[test]
fn explicit_native_fresh_update_adopt_and_snapshot_identity() {
    let f = Fixture::new();
    let a = f.account("a");
    f.post(&a, 1, ALICE, "fresh-native", "");
    let out = f.root.path().join("native");
    success(f.native(&a, &out, &[]));
    let dir = out.join("Alice/SNS");
    consistent(&dir, 1);
    assert!(
        !dir.join("_source_binding.json").exists(),
        "默认 fresh 保持原输出格式，不建立绑定"
    );
    let before = tree(&out);
    rejected(f.native(&a, &out, &[]));
    preserved(&out, &before);
    rejected(f.native(&a, &out, &["--adopt-existing"]));
    preserved(&out, &before);
    rejected(f.native(&a, &out, &["--update"]));
    preserved(&out, &before);
    success(f.native(&a, &out, &["--update", "--adopt-existing"]));
    let binding = read_json(&dir.join("_source_binding.json"));
    assert_eq!(binding["source_kind"], "snapshot");
    assert_eq!(binding["source_id"].as_str().unwrap().len(), 64);
    f.post(&a, 1, ALICE, "native-updated", "");
    success(f.native(&a, &out, &["--update"]));
    assert!(consistent(&dir, 1).to_string().contains("native-updated"));
    assert_eq!(read_json(&dir.join("_source_binding.json")), binding);
    // 从首次就显式更新的新输出建立绑定，后续无需认领。
    let bound = f.root.path().join("native-bound-from-start");
    success(f.native(&a, &bound, &["--update"]));
    assert_eq!(
        read_json(&bound.join("Alice/SNS/_source_binding.json"))["source_id"],
        binding["source_id"]
    );
    f.post(&a, 1, ALICE, "bound-second-round", "");
    success(f.native(&a, &bound, &["--update"]));
    assert!(consistent(&bound.join("Alice/SNS"), 1)
        .to_string()
        .contains("bound-second-round"));
    let legacy = f.root.path().join("native-legacy");
    fs::create_dir_all(legacy.join("Alice/SNS")).unwrap();
    fs::write(legacy.join("Alice/SNS/keep.bin"), b"legacy").unwrap();
    let before = tree(&legacy);
    rejected(f.native(&a, &legacy, &["--update"]));
    preserved(&legacy, &before);
    success(f.native(&a, &legacy, &["--update", "--adopt-existing"]));
    assert_eq!(
        fs::read(legacy.join("Alice/SNS/keep.bin")).unwrap(),
        b"legacy"
    );
    // 同数据库配不同联系人数据库，必须属于不同静态来源。
    let alternate = a.join("decrypted/contact/alternate.db");
    fs::copy(a.join("decrypted/contact/contact.db"), &alternate).unwrap();
    let sns = a.join("decrypted/sns/sns.db");
    let before = tree(&out);
    rejected(f.run(
        &a,
        &[
            "toolkit",
            "export-sns-native",
            sns.to_str().unwrap(),
            out.to_str().unwrap(),
            "--contact-db",
            alternate.to_str().unwrap(),
            "--contacts",
            ALICE,
            "--update",
            "--adopt-existing",
        ],
    ));
    preserved(&out, &before);
}

struct Server {
    url: String,
    body: Arc<Mutex<Vec<u8>>>,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/synthetic.png", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let body = Arc::new(Mutex::new(png(1)));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (payload, count, done) = (body.clone(), requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let _ = stream.read(&mut request);
                        count.fetch_add(1, Ordering::SeqCst);
                        let body = payload.lock().unwrap().clone();
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(&body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("本机监听失败: {e}"),
                }
            }
        });
        Self {
            url,
            body,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn png(marker: u8) -> Vec<u8> {
    use base64::Engine;
    let mut bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aC1sAAAAASUVORK5CYII=").unwrap();
    bytes.resize(128, marker);
    bytes
}

#[test]
fn cli_contacts_override_environment_and_no_remote_blocks_environment_downloads() {
    let server = Server::new();
    let f = Fixture::new();
    let a = f.account("environment");
    let media = format!(
        "<media><id>environment-image</id><type>2</type><url>{}</url></media>",
        server.url
    );
    f.post(&a, 1, ALICE, "environment-alice", &media);
    f.post(&a, 2, BOB, "environment-bob", &media);
    for (name, cli_contacts, alice, bob) in [
        ("env-only", None, false, true),
        ("cli-wins", Some(ALICE), true, false),
        ("explicit-empty", Some(""), true, true),
    ] {
        let out = f.root.path().join(name);
        let mut args = vec![
            "toolkit",
            "export-sns",
            "--output-dir",
            out.to_str().unwrap(),
            "--no-remote",
        ];
        if let Some(contacts) = cli_contacts {
            args.extend(["--contacts", contacts]);
        }
        let report = success(f.run_env(
            &a,
            &args,
            &[
                ("WECHAT_EXPORT_CONTACTS", BOB),
                ("WECHAT_SNS_DOWNLOAD_MEDIA", "1"),
            ],
        ));
        let count = usize::from(alice) + usize::from(bob);
        assert_eq!(report["contacts"], count);
        assert_eq!(report["posts"], count);
        assert_eq!(report["media_downloaded"], 0);
        for (display, selected) in [("Alice", alice), ("Bob", bob)] {
            let dir = out.join(display).join("SNS");
            if selected {
                let timeline = consistent(&dir, 1);
                assert!(timeline["posts"][0]["media"][0].get("local_file").is_none());
                assert!(!fs::read_to_string(dir.join("timeline.html"))
                    .unwrap()
                    .contains("src=\"http"));
            } else {
                assert!(
                    !out.join(display).exists(),
                    "被排除联系人不得产生输出或绑定"
                );
            }
        }
        assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    }
    // 正向对照：同一媒体与子进程环境不带 no-remote 时确实能发起下载。
    let out = f.root.path().join("environment-download-control");
    let report = success(f.run_env(
        &a,
        &[
            "toolkit",
            "export-sns",
            "--output-dir",
            out.to_str().unwrap(),
            "--contacts",
            ALICE,
        ],
        &[("WECHAT_SNS_DOWNLOAD_MEDIA", "1")],
    ));
    assert_eq!(report["media_downloaded"], 1);
    let timeline = consistent(&out.join("Alice/SNS"), 1);
    let file = timeline["posts"][0]["media"][0]["local_file"]
        .as_str()
        .unwrap();
    assert_eq!(fs::read(out.join("Alice/SNS").join(file)).unwrap(), png(1));
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}

fn media_audit_consistent(dir: &Path, timeline: &Value, statuses: &[&str]) {
    let audit = read_json(&dir.join("_media_recovery.json"));
    let entries = audit.as_array().unwrap();
    let posts = timeline["posts"].as_array().unwrap();
    assert_eq!(entries.len(), posts.len());
    for post in posts {
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| read_json(&dir.join(entry["post_file"].as_str().unwrap())) == *post)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "每条汇总帖子必须恰好对应一条审计及同内容单帖"
        );
        let post_file = matches[0]["post_file"].as_str().unwrap();
        let stem = Path::new(post_file).file_stem().unwrap().to_str().unwrap();
        let html = fs::read_to_string(dir.join("timeline.html")).unwrap();
        let items = matches[0]["recovery"]["media"].as_array().unwrap();
        assert_eq!(items.len(), statuses.len());
        for (index, (item, expected)) in items.iter().zip(statuses).enumerate() {
            assert_eq!(item["media_index"], index);
            assert_eq!(item["status"], *expected);
            if let Some(file) = post["media"][index]["local_file"].as_str() {
                // 配置宿主保留 legacy 平铺布局；不能把缓存子目录引用泄漏到最终产物。
                let extension = Path::new(file).extension().unwrap().to_str().unwrap();
                assert_eq!(file, format!("{stem}_{index}.{extension}"));
                assert!(
                    html.contains(&format!("src=\"{file}\"")),
                    "HTML 必须引用同一个平铺媒体文件"
                );
                assert_eq!(item["reference"]["local_file"], file);
                assert_eq!(item["bytes"], fs::metadata(dir.join(file)).unwrap().len());
            } else {
                assert!(item["reference"].is_null());
            }
        }
    }
}

#[test]
fn cache_and_loopback_changes_replace_owned_media_without_deleting_unrelated_files() {
    let server = Server::new();
    let f = Fixture::new();
    let a = f.account("a");
    let xml = format!("<media><id>cached-video</id><type>6</type></media><media><id>remote-image</id><type>2</type><url>{}</url></media>", server.url);
    f.post(&a, 1, ALICE, "media-post", &xml);
    let hash = format!("{:x}", md5::compute("1_cached-video_3"));
    let cache = a.join("cache");
    let video = cache
        .join("2024-03/Sns/Video")
        .join(&hash[..2])
        .join(format!("{}.mp4", &hash[2..]));
    fs::create_dir_all(video.parent().unwrap()).unwrap();
    let first = b"\x00\x00\x00\x18ftypmp42synthetic-first";
    let second = b"\x00\x00\x00\x18ftypmp42synthetic-second";
    fs::write(&video, first).unwrap();
    let out = f.root.path().join("media");
    let flags = ["--download-media"];
    let report = success(f.alias(&a, Some(&out), ALICE, &flags));
    assert_eq!(report["media_downloaded"], 1);
    assert_eq!(report["media_recovered"], 2);
    let dir = out.join("Alice/SNS");
    let timeline = consistent(&dir, 1);
    media_audit_consistent(&dir, &timeline, &["recovered", "downloaded"]);
    let media = &timeline["posts"][0]["media"];
    let cached_name = media[0]["local_file"].as_str().unwrap();
    let remote_name = media[1]["local_file"].as_str().unwrap();
    assert_eq!(Path::new(cached_name).extension().unwrap(), "mp4");
    assert_eq!(Path::new(remote_name).extension().unwrap(), "png");
    assert_eq!(fs::read(dir.join(cached_name)).unwrap(), first);
    assert_eq!(fs::read(dir.join(remote_name)).unwrap(), png(1));
    fs::write(dir.join("unrelated-old.png"), b"unrelated").unwrap();
    fs::write(&video, second).unwrap();
    *server.body.lock().unwrap() = png(2);
    let report = success(f.alias(&a, Some(&out), ALICE, &flags));
    assert_eq!(report["media_downloaded"], 1);
    assert_eq!(report["media_recovered"], 2);
    assert_eq!(report["media_failed"], 0);
    let updated = consistent(&dir, 1);
    media_audit_consistent(&dir, &updated, &["recovered", "downloaded"]);
    assert_eq!(updated["posts"][0]["media"][0]["local_file"], cached_name);
    assert_eq!(updated["posts"][0]["media"][1]["local_file"], remote_name);
    assert_eq!(fs::read(dir.join(cached_name)).unwrap(), second);
    assert_eq!(fs::read(dir.join(remote_name)).unwrap(), png(2));
    assert_eq!(
        fs::read(dir.join("unrelated-old.png")).unwrap(),
        b"unrelated"
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    // 使用全新目录证明 no-remote 不是因旧目标命中才没有访问网络。
    let offline = f.root.path().join("offline");
    success(f.alias(&a, Some(&offline), ALICE, &["--no-remote"]));
    let timeline = consistent(&offline.join("Alice/SNS"), 1);
    media_audit_consistent(
        &offline.join("Alice/SNS"),
        &timeline,
        &["recovered", "missing"],
    );
    assert!(timeline["posts"][0]["media"][0]["local_file"].is_string());
    assert!(timeline["posts"][0]["media"][1].get("local_file").is_none());
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
}
