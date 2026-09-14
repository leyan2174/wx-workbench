//! 真实进程与合成账号，不包含或替代生产 query/protocol。
pub mod attachments;
pub(crate) mod encrypted_sqlite;
pub mod history;
#[path = "../../../src/toolkit/private_file.rs"]
#[allow(dead_code)]
mod private_file;
#[path = "../../support/bootstrap.rs"]
mod runtime_cleanup;
use encrypted_sqlite::{encrypt, sqlite};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Account {
    root: tempfile::TempDir,
    home: PathBuf,
    pipe: String,
    daemon: Option<Child>,
    pub marker: &'static str,
}

impl Account {
    pub fn new(home: &Path, marker: &'static str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(home).unwrap();
        let storage = root.path().join("db_storage");
        for folder in ["contact", "message"] {
            fs::create_dir_all(storage.join(folder)).unwrap();
        }
        let mut keys = serde_json::Map::new();
        let plain = root.path().join("build.db");
        let conn = sqlite(&plain);
        conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,extra_buffer BLOB); CREATE TABLE contact_label(label_id_,label_name_,sort_order_)").unwrap();
        let ids = if marker == "A" { "1,1" } else { "1" };
        let mut buffer = vec![242u8, 1, ids.len() as u8];
        buffer.extend_from_slice(ids.as_bytes());
        conn.execute(
            "INSERT INTO contact VALUES('peer',?1,'',0,?2)",
            rusqlite::params![format!("姓名{marker}"), buffer],
        )
        .unwrap();
        conn.execute_batch("INSERT INTO contact VALUES('other','Other','',0,NULL)")
            .unwrap();
        conn.execute(
            "INSERT INTO contact_label VALUES(1,?1,1),(2,'',2)",
            [format!("标签{marker}")],
        )
        .unwrap();
        drop(conn);
        encrypt(&plain, &storage.join("contact/contact.db"));
        keys.insert("contact/contact.db".into(), json!("11".repeat(32)));
        let table = format!("Msg_{:x}", md5::compute(b"peer"));
        for shard in 0..2 {
            let conn = sqlite(&plain);
            conn.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other'); CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")).unwrap();
            for (id, timestamp) in if shard == 0 {
                vec![(7, 100), (8, 200)]
            } else {
                vec![(8, 201)]
            } {
                let body = format!("<msg><appmsg><title>reply-{marker}-{timestamp}</title><type>57</type><refermsg><type>1</type><fromusr>peer</fromusr><chatusr>peer</chatusr><displayname>original-sender</displayname><content>original-{marker}</content><svrid>700</svrid><createtime>90</createtime></refermsg></appmsg></msg>");
                conn.execute(
                    &format!("INSERT INTO [{table}] VALUES(?1,49,?2,7,?3,0)"),
                    rusqlite::params![id, timestamp, body],
                )
                .unwrap();
            }
            attachments::seed(&conn, &table, shard, root.path(), marker);
            history::seed(&conn, &table, shard, marker);
            drop(conn);
            let key = format!("message/message_{shard}.db");
            encrypt(&plain, &storage.join(&key));
            keys.insert(key, json!("11".repeat(32)));
        }
        let conn = sqlite(&plain);
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,voice_data BLOB); INSERT INTO VoiceInfo(rowid,chat_name_id,local_id,create_time,voice_data) VALUES(1,7,7,100,NULL),(3,8,999,999,x'ff')").unwrap();
        conn.execute("INSERT INTO VoiceInfo(rowid,chat_name_id,local_id,create_time,voice_data) VALUES(2,7,8,110,?1)", [vec![0x41u8; if marker == "A" {3} else {5}]]).unwrap();
        drop(conn);
        encrypt(&plain, &storage.join("message/media_0.db"));
        keys.insert("message/media_0.db".into(), json!("11".repeat(32)));
        fs::write(
            root.path().join("keys.json"),
            serde_json::to_vec(&keys).unwrap(),
        )
        .unwrap();
        fs::write(root.path().join("config.json"), json!({"db_dir":storage,"keys_file":root.path().join("keys.json"),"decrypted_dir":root.path().join("decrypted")}).to_string()).unwrap();
        let mut hash = Sha256::new();
        hash.update(b"wx-cli-runtime-v2\0");
        for path in [
            root.path().join("config.json"),
            storage,
            root.path().join("keys.json"),
            home.to_owned(),
        ] {
            hash.update(
                path.canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .to_lowercase()
                    .as_bytes(),
            );
            hash.update([0]);
        }
        Self {
            root,
            home: home.into(),
            pipe: format!("wx-cli-v2-{:x}", hash.finalize()),
            daemon: None,
            marker,
        }
    }

    fn command(&self) -> Command {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wx"));
        cmd.env_clear()
            .current_dir(self.root.path())
            .env("PATH", "")
            .env("WX_CLI_CONFIG", self.root.path().join("config.json"))
            .env("WX_CLI_HOME", &self.home)
            .creation_flags(0x08000000);
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            cmd.env(key, &self.home);
        }
        cmd
    }

    pub fn start(&mut self) {
        let log = fs::File::create(self.root.path().join("daemon.log")).unwrap();
        let mut cmd = self.command();
        cmd.env("WX_DAEMON_MODE", "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        println!("COMMAND: {cmd:?}");
        println!(
            "CWD: {:?}; ENV (env_clear): {:?}",
            cmd.get_current_dir(),
            cmd.get_envs().collect::<Vec<_>>()
        );
        self.daemon = Some(cmd.spawn().unwrap());
        self.register_daemon_identity();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(pong) = self.ipc(json!({"cmd":"ping"})) {
                assert_eq!(pong, json!({"ok":true,"pong":true}));
                return;
            }
            assert!(
                self.daemon.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon 提前退出"
            );
            assert!(Instant::now() < deadline, "daemon 未及时就绪");
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn register_daemon_identity(&self) {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::{FILETIME, HANDLE},
            System::Threading::GetProcessTimes,
        };
        let child = self.daemon.as_ref().unwrap();
        let mut created = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetProcessTimes(
                HANDLE(child.as_raw_handle()),
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            )
            .unwrap();
        }
        let runtime_id = self.pipe.strip_prefix("wx-cli-v2-").unwrap();
        let directory = self.home.join("accounts").join(runtime_id);
        fs::create_dir_all(&directory).unwrap();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(directory.join("daemon.pid"))
            .unwrap();
        private_file::restrict(&file).unwrap();
        let record = json!({"pid":child.id(), "exe":env!("CARGO_BIN_EXE_wx"),
            "created":(u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
            "runtime_id":runtime_id});
        file.write_all(&serde_json::to_vec(&record).unwrap())
            .unwrap();
        file.sync_all().unwrap();
    }

    pub fn ipc(&self, request: Value) -> Result<Value, String> {
        use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced};
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(3), async {
                    let name = self
                        .pipe
                        .as_str()
                        .to_ns_name::<GenericNamespaced>()
                        .map_err(|e| e.to_string())?;
                    let mut socket = interprocess::local_socket::tokio::Stream::connect(name)
                        .await
                        .map_err(|e| e.to_string())?;
                    println!("IPC {} REQUEST: {request}", self.marker);
                    socket
                        .write_all(format!("{request}\n").as_bytes())
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut text = String::new();
                    tokio::io::BufReader::new(socket.take(1024 * 1024))
                        .read_line(&mut text)
                        .await
                        .map_err(|e| e.to_string())?;
                    println!("IPC RESPONSE: {text}");
                    serde_json::from_str(&text).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            })
    }

    pub fn mcp(&self) -> Mcp {
        self.mcp_with_args(&[])
    }

    pub fn mcp_with_args(&self, args: &[&str]) -> Mcp {
        let mut cmd = self.command();
        let log_path = self.root.path().join("mcp-stderr.log");
        cmd.arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&log_path).unwrap());
        println!("COMMAND: {cmd:?}");
        println!(
            "CWD: {:?}; ENV (env_clear): {:?}",
            cmd.get_current_dir(),
            cmd.get_envs().collect::<Vec<_>>()
        );
        let mut child = cmd.spawn().unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut output = BufReader::new(output.take(4 * 1024 * 1024));
            loop {
                let mut line = Vec::new();
                if output.read_until(b'\n', &mut line).unwrap() == 0 {
                    break;
                }
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Mcp {
            child,
            input,
            receive,
            reader: Some(reader),
            log_path,
            next_id: 1,
        }
    }

    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        fn visit(root: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for item in fs::read_dir(root).unwrap() {
                let path = item.unwrap().path();
                if path.is_dir() {
                    result.insert(path.clone(), Vec::new());
                    visit(&path, result);
                } else {
                    result.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        for folder in ["db_storage", "msg"] {
            visit(&self.root.path().join(folder), &mut result);
        }
        for file in ["config.json", "keys.json"] {
            let path = self.root.path().join(file);
            result.insert(path.clone(), fs::read(path).unwrap());
        }
        result
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            println!("DAEMON EXIT: {:?}", child.wait());
            println!(
                "DAEMON STDOUT/STDERR:\n{}",
                fs::read_to_string(self.root.path().join("daemon.log")).unwrap_or_default()
            );
        }
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        drop(runtime_cleanup::RuntimeCleanup(self.home.clone()));
        self.stop();
    }
}

pub struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    receive: mpsc::Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    log_path: PathBuf,
    next_id: u64,
}
impl Mcp {
    pub fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        println!("MCP REQUEST: {request}");
        writeln!(self.input.as_mut().unwrap(), "{request}").unwrap();
        let bytes = self
            .receive
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP 请求超时");
        println!("MCP STDOUT: {}", String::from_utf8_lossy(&bytes));
        assert!(bytes.ends_with(b"\n"));
        let reply: Value = serde_json::from_slice(&bytes).expect("stdout 必须只含 JSON-RPC");
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        reply
    }
    pub fn ready(&mut self) {
        let reply = self.rpc("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"readonly-real-daemon","version":"1"}}));
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        writeln!(
            self.input.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
    }
    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":arguments}))
    }
    pub fn data(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.call(name, arguments);
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    pub fn finish(mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "MCP EOF 后未退出");
            thread::sleep(Duration::from_millis(10));
        }
        self.reader.take().unwrap().join().unwrap();
        assert!(self.receive.try_iter().next().is_none(), "多余协议输出");
        let stderr = fs::read_to_string(&self.log_path).unwrap();
        println!("MCP STDERR: {stderr}");
        assert!(stderr.is_empty());
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        self.input.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        println!(
            "MCP FINAL STDERR: {}",
            fs::read_to_string(&self.log_path).unwrap_or_default()
        );
    }
}
