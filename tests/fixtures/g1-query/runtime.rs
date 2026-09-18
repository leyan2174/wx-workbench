use crate::support::Account;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct Fixture {
    pub account: Account,
    // Account must be dropped before its runtime home.
    _home: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    pub fn new(marker: &'static str, extended: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        let account = Account::new(home.path(), marker);
        if extended {
            super::seed::extend(&account);
        }
        // Preserve the canonical runtime anchor, but leave no legacy keys to read.
        // The shared helper has already written the current DPAPI store.
        fs::write(account.root().join("keys.json"), b"{}").unwrap();
        Self {
            account,
            home: home.path().into(),
            _home: home,
        }
    }

    pub fn command(&self) -> Command {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .env_clear()
            .current_dir(self.account.root())
            .env("PATH", "")
            .env("WX_CLI_CONFIG", self.account.root().join("config.json"))
            .env("WX_CLI_HOME", &self.home)
            .creation_flags(0x08000000)
            .stdin(Stdio::null());
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            command.env(key, &self.home);
        }
        command
    }

    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut snapshot = self.account.snapshot();
        let config: Value =
            serde_json::from_slice(&fs::read(self.account.root().join("config.json")).unwrap())
                .unwrap();
        let path = PathBuf::from(
            config["key_store"]
                .as_str()
                .expect("DPAPI fixture required"),
        );
        assert!(path
            .canonicalize()
            .unwrap()
            .starts_with(self.account.root().canonicalize().unwrap()));
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"WXKEYS\0\x01"));
        snapshot.insert(path, bytes);
        snapshot
    }

    pub fn cli(&self, args: &[&str]) -> CliOutput {
        // Files prevent a full stdout pipe from deadlocking a bounded wait.
        let stdout = tempfile::NamedTempFile::new_in(self.account.root()).unwrap();
        let stderr = tempfile::NamedTempFile::new_in(self.account.root()).unwrap();
        let mut command = self.command();
        command
            .args(args)
            .arg("--json")
            .stdout(stdout.reopen().unwrap())
            .stderr(stderr.reopen().unwrap());
        println!("COMMAND: {command:?}");
        let mut child = Process(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "CLI timed out: {args:?}");
            thread::sleep(Duration::from_millis(20));
        };
        CliOutput {
            status,
            stdout: fs::read(stdout.path()).unwrap(),
            stderr: fs::read_to_string(stderr.path()).unwrap(),
        }
    }

    pub fn data(&self, request: Value) -> Value {
        let mut reply = self.account.ipc(request).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(reply.get("error").is_none(), "{reply}");
        reply.as_object_mut().unwrap().remove("ok");
        reply
    }
}

pub struct CliOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}
impl CliOutput {
    pub fn data(self) -> Value {
        assert!(self.status.success(), "CLI failed: {}", self.stderr);
        serde_json::from_slice(&self.stdout).expect("CLI stdout must be JSON")
    }
    pub fn failed(self) {
        assert!(
            !self.status.success(),
            "CLI falsely succeeded: {:?}",
            self.stdout
        );
        assert!(!self.stderr.trim().is_empty(), "missing CLI diagnostic");
    }
}

pub struct Process(pub Child);
impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

pub struct Web {
    process: Process,
    pub origin: String,
    pub token: String,
    http: reqwest::blocking::Client,
}
impl Web {
    pub fn start(fixture: &Fixture) -> Self {
        let path = fixture.account.root().join("g1-web.log");
        let log = fs::File::create(&path).unwrap();
        let mut command = fixture.command();
        command
            .args(["web", "--port", "0"])
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        println!("COMMAND: {command:?}");
        let mut process = Process(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        let (origin, token) = loop {
            let text = fs::read_to_string(&path).unwrap();
            if let Some(start) = text.find("http://127.0.0.1:") {
                if let Some((origin, token)) = text[start..]
                    .lines()
                    .next()
                    .unwrap()
                    .trim()
                    .split_once("/#token=")
                {
                    break (origin.to_owned(), token.to_owned());
                }
            }
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "Web exited: {text}"
            );
            assert!(Instant::now() < deadline, "Web startup timed out: {text}");
            thread::sleep(Duration::from_millis(30));
        };
        assert!(!token.is_empty());
        let url = reqwest::Url::parse(&origin).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.port().is_some_and(|port| port > 0));
        Self {
            process,
            origin,
            token,
            http: reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }

    pub fn get(&self, path: &str) -> (u16, Value) {
        assert!(path.starts_with("/api/"));
        let response = self
            .http
            .get(format!("{}{path}", self.origin))
            .header("x-wx-token", &self.token)
            .header("origin", &self.origin)
            .send()
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or_else(|_| panic!("HTTP {status}: {text}")),
        )
    }

    pub fn data(&self, path: &str) -> Value {
        let (status, value) = self.get(path);
        assert_eq!(status, 200, "{path}: {value}");
        value
    }

    pub fn assert_running(&mut self) {
        assert!(
            self.process.0.try_wait().unwrap().is_none(),
            "Web exited during manual inspection"
        );
    }
}

// Remove the published rendezvous on success, timeout, and assertion unwind.
pub struct Rendezvous(pub PathBuf, pub PathBuf);
impl Drop for Rendezvous {
    fn drop(&mut self) {
        for path in [&self.0, &self.1] {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn publish(path: &Path, web: &Web) -> Rendezvous {
    use std::io::Write;
    assert!(path.is_absolute(), "WX_G1_UI_FIXTURE_INFO must be absolute");
    let stop = path.with_extension("stop");
    assert!(
        !stop.exists(),
        "remove stale synthetic stop marker first: {}",
        stop.display()
    );
    // create_new never overwrites another running fixture's rendezvous.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    let cleanup = Rendezvous(path.into(), stop.clone());
    let value = serde_json::json!({"url":format!("{}/#token={}", web.origin, web.token),
        "origin":web.origin,"token":web.token,"stop_file":stop,
        "synthetic":true,"max_wait_seconds":600,"pid":web.process.0.id()});
    file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    cleanup
}
