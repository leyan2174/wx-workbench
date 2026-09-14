//! Built-binary acceptance for transient, daemon-owned foreground operations.
//! Synthetic inputs only; independent of production module imports and account fixtures.
#![cfg(windows)]

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::windows::{ffi::OsStringExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ClientOptions,
};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
                WaitForSingleObject, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            },
        },
    },
};

#[link(name = "kernel32")]
extern "system" {
    fn GetNamedPipeServerProcessId(pipe: *mut std::ffi::c_void, pid: *mut u32) -> i32;
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn process(pid: u32) -> Result<Handle> {
    Ok(Handle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            false,
            pid,
        )?
    }))
}

fn birth(handle: &Handle) -> Result<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user)?;
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

#[derive(Debug, PartialEq)]
struct Streams {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct Fixture {
    root: tempfile::TempDir,
    rt: tokio::runtime::Runtime,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("account")).unwrap();
        Self {
            root,
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    fn environment(&self) -> BTreeMap<String, String> {
        let mut result = BTreeMap::new();
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "USERPROFILE"] {
            if let Ok(value) = std::env::var(name) {
                result.insert(name.into(), value);
            }
        }
        for (name, value) in [
            (
                "WX_CLI_CONFIG",
                self.root.path().join("account/config.json"),
            ),
            ("WX_CLI_HOME", self.root.path().join("home")),
            (
                "WX_WECHAT_DECRYPT_PYTHON",
                self.root.path().join("absent-python.exe"),
            ),
            (
                "WX_WECHAT_DECRYPT_DIR",
                self.root.path().join("absent-toolkit"),
            ),
        ] {
            result.insert(name.into(), value.to_string_lossy().into_owned());
        }
        result.insert("PATH".into(), String::new());
        result
    }

    fn cli(&self, cwd: &Path, args: &[&str], environment: &BTreeMap<String, String>) -> Streams {
        let out = tempfile::NamedTempFile::new_in(self.root.path()).unwrap();
        let err = tempfile::NamedTempFile::new_in(self.root.path()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(environment)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(out.reopen().unwrap())
            .stderr(err.reopen().unwrap());
        println!("COMMAND: {command:?}");
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(40);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "CLI timed out; stdout={:?}; stderr={:?}",
                    fs::read(out.path()).unwrap(),
                    fs::read(err.path()).unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let streams = Streams {
            code: status.code().unwrap_or(-1),
            stdout: fs::read(out.path()).unwrap(),
            stderr: fs::read(err.path()).unwrap(),
        };
        println!(
            "EXIT: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            streams.code,
            String::from_utf8_lossy(&streams.stdout),
            String::from_utf8_lossy(&streams.stderr)
        );
        streams
    }

    fn directory(&self) -> Result<PathBuf> {
        let paths: Vec<_> = fs::read_dir(self.root.path().join("home/bootstrap"))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.join("daemon.pid").is_file())
            .collect();
        ensure!(
            paths.len() == 1,
            "expected exactly one test bootstrap daemon: {paths:?}"
        );
        Ok(paths[0].clone())
    }

    fn identity(&self) -> Value {
        serde_json::from_slice(&fs::read(self.directory().unwrap().join("daemon.pid")).unwrap())
            .unwrap()
    }

    fn boot(&self) {
        let output = self.cli(
            self.root.path(),
            &["toolkit", "status", "--json"],
            &self.environment(),
        );
        assert_eq!(output.code, 0, "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(!self.root.path().join("account/config.json").exists());
        self.directory().unwrap();
    }

    fn verified_process(&self, record: &Value) -> Result<Handle> {
        let handle = process(record["pid"].as_u64().context("missing pid")? as u32)?;
        ensure!(
            birth(&handle)? == record["created"].as_u64().context("missing birth")?,
            "PID reused"
        );
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_FORMAT(0),
                PWSTR(name.as_mut_ptr()),
                &mut length,
            )?;
        }
        let actual = PathBuf::from(std::ffi::OsString::from_wide(&name[..length as usize]))
            .canonicalize()?;
        ensure!(
            actual == Path::new(env!("CARGO_BIN_EXE_wx")).canonicalize()?,
            "not test wx binary"
        );
        Ok(handle)
    }

    fn call_with_token(&self, request: Value, wrong_token: bool) -> Result<Value> {
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let directory = self.directory()?;
                let record: Value =
                    serde_json::from_slice(&fs::read(directory.join("daemon.pid"))?)?;
                let _handle = self.verified_process(&record)?;
                let id = record["runtime_id"]
                    .as_str()
                    .context("missing runtime id")?;
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut pipe = loop {
                    match ClientOptions::new().open(format!(r"\\.\pipe\wx-cli-tasks-v1-{id}")) {
                        Ok(pipe) => break pipe,
                        Err(error)
                            if error.raw_os_error() == Some(231) && Instant::now() < deadline =>
                        {
                            tokio::time::sleep(Duration::from_millis(10)).await
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                let mut server = 0;
                ensure!(
                    unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server) } != 0,
                    "pipe identity unavailable"
                );
                ensure!(
                    u64::from(server) == record["pid"].as_u64().unwrap(),
                    "pipe PID mismatch"
                );
                let token = if wrong_token {
                    "0".repeat(64)
                } else {
                    fs::read_to_string(directory.join("service-token.key"))?
                };
                let encoded = serde_json::to_vec(
                    &json!({"version":1,"runtime_id":id,"token":token,"request":request}),
                )?;
                pipe.write_u32_le(encoded.len() as u32).await?;
                pipe.write_all(&encoded).await?;
                let length = pipe.read_u32_le().await? as usize;
                ensure!(
                    length > 0 && length <= 8 * 1024 * 1024,
                    "invalid reply length"
                );
                let mut bytes = vec![0; length];
                pipe.read_exact(&mut bytes).await?;
                pipe.write_all(&[0]).await?;
                let reply: Value = serde_json::from_slice(&bytes)?;
                ensure!(
                    reply["version"] == 1 && reply["runtime_id"] == id,
                    "wrong reply identity"
                );
                Ok(reply)
            })
            .await
            .context("test RPC timed out")?
        })
    }

    fn call(&self, request: Value) -> Value {
        self.call_with_token(request, false).unwrap()
    }
    fn ok(&self, request: Value) -> Value {
        let reply = self.call(request);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    }
    fn start(
        &self,
        id: &str,
        operation: Value,
        cwd: &Path,
        environment: &BTreeMap<String, String>,
    ) {
        self.ok(json!({"op":"operation_start","id":id,"invocation":{
            "operation":operation,"cwd":cwd.canonicalize().unwrap(),"environment":environment}}));
    }
    fn finish(&self, id: &str) -> Streams {
        let (mut stdout, mut stderr, mut after) = (Vec::new(), Vec::new(), 0u64);
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            assert!(Instant::now() < deadline, "operation never finished");
            let page = self.ok(json!({"op":"operation_poll","id":id,"after":after}));
            for chunk in page["chunks"].as_array().unwrap() {
                assert_eq!(chunk["seq"].as_u64(), Some(after + 1));
                let bytes: Vec<u8> = serde_json::from_value(chunk["bytes"].clone()).unwrap();
                if chunk["stderr"].as_bool().unwrap() {
                    stderr.extend(bytes);
                } else {
                    stdout.extend(bytes);
                }
                after += 1;
            }
            if let Some(code) = page["exit_code"].as_i64() {
                self.ok(json!({"op":"operation_cancel","id":id}));
                return Streams {
                    code: code as i32,
                    stdout,
                    stderr,
                };
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let Ok(directory) = self.directory() else {
            return;
        };
        let Ok(bytes) = fs::read(directory.join("daemon.pid")) else {
            return;
        };
        let Ok(record) = serde_json::from_slice(&bytes) else {
            return;
        };
        let Ok(handle) = self.verified_process(&record) else {
            return;
        };
        let _ = self.call_with_token(json!({"op":"shutdown"}), false);
        if unsafe { WaitForSingleObject(handle.0, 5000) } != WAIT_OBJECT_0 {
            eprintln!(
                "Test-owned daemon required fallback termination after authenticated shutdown"
            );
            let _ = unsafe { TerminateProcess(handle.0, 1) };
            let _ = unsafe { WaitForSingleObject(handle.0, 5000) };
        }
    }
}

fn status() -> Value {
    json!({"kind":"toolkit","args":{"operation":{"kind":"status","args":{"json":true}}}})
}

#[test]
fn missing_config_bootstraps_and_cli_preserves_service_stdout_stderr_and_exit() {
    let f = Fixture::new();
    f.boot();
    let record = f.identity();
    let env = f.environment();
    for (id, operation, args, expected) in [
        (
            "11".repeat(32),
            status(),
            vec!["toolkit", "status", "--json"],
            0,
        ),
        (
            "22".repeat(32),
            json!({"kind":"transcribe_audio","args":{"args":{
            "input":"missing.wav","backend":backend(Some(Path::new("absent.exe")))}}}),
            vec![
                "toolkit",
                "transcribe-audio-native",
                "missing.wav",
                "--whisper-binary",
                "absent.exe",
                "--whisper-model",
                "model.bin",
            ],
            1,
        ),
    ] {
        f.start(&id, operation, f.root.path(), &env);
        let rpc = f.finish(&id);
        let cli = f.cli(f.root.path(), &args, &env);
        assert_eq!(
            cli, rpc,
            "CLI must not add error wrapping or startup chatter"
        );
        assert_eq!(cli.code, expected);
        if expected == 0 {
            assert!(cli.stderr.is_empty());
        } else {
            assert!(cli.stdout.is_empty());
            assert!(!cli.stderr.is_empty());
        }
        assert_eq!(f.identity(), record);
    }
}

#[test]
fn shared_bootstrap_preserves_each_callers_relative_paths_and_environment() {
    let f = Fixture::new();
    let mut initial = f.environment();
    initial.insert("WX_WECHAT_DECRYPT_DIR".into(), "starter-only".into());
    assert_eq!(
        f.cli(f.root.path(), &["toolkit", "status", "--json"], &initial)
            .code,
        0
    );
    let record = f.identity();
    for (name, count, override_value) in [
        ("a", 1, Some("caller-a")),
        ("b", 3, None),
        ("a", 1, Some("caller-a-again")),
    ] {
        let cwd = f.root.path().join(name);
        fs::create_dir_all(cwd.join("chosen")).unwrap();
        fs::write(
            cwd.join("chosen/chat_transcribed.json"),
            json!({"messages":vec![json!({"type":"voice","transcription":"synthetic"}); count]})
                .to_string(),
        )
        .unwrap();
        let mut env = f.environment();
        env.remove("WX_WECHAT_DECRYPT_DIR");
        if let Some(value) = override_value {
            env.insert("WX_WECHAT_DECRYPT_DIR".into(), value.into());
        }
        let output = f.cli(&cwd, &["toolkit", "status", "--json"], &env);
        assert_eq!(output.code, 0);
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            status["env_overrides"]["wx_wechat_decrypt_dir"],
            json!(override_value)
        );
        let output = f.cli(
            &cwd,
            &[
                "toolkit",
                "run",
                "status",
                "--",
                "--json",
                "--exported-dir",
                "chosen",
            ],
            &env,
        );
        assert_eq!(output.code, 0, "{output:?}");
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            status["progress"],
            json!({"voices":count,"transcribed":count})
        );
        assert_eq!(
            Path::new(status["exported_dir"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            cwd.join("chosen").canonicalize().unwrap()
        );
        assert_eq!(f.identity(), record);
    }
}

#[test]
fn authenticated_api_rejects_arbitrary_commands_and_internal_environment() {
    let f = Fixture::new();
    f.boot();
    let id = "33".repeat(32);
    let valid = json!({"op":"operation_start","id":id,"invocation":{
        "operation":status(),"cwd":f.root.path().canonicalize().unwrap(),"environment":f.environment()}});
    let reply = f.call_with_token(valid.clone(), true).unwrap();
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "unauthorized");
    for operation in [
        json!({"kind":"run","args":{"command":"cmd.exe","argv":["/c","exit","0"]}}),
        json!({"kind":"toolkit","args":{"operation":{"kind":"run","args":{"command":"cmd.exe"}}}}),
        json!({"kind":"toolkit","args":{"operation":{"kind":"status","args":{"json":true,"argv":[]}}}}),
        json!({"kind":"transcribe_audio","args":{"args":{"input":"missing.wav","backend":backend(None)}}}),
    ] {
        let mut request = valid.clone();
        request["invocation"]["operation"] = operation;
        assert_eq!(f.call(request)["ok"], false);
    }
    for name in [
        "WX_DAEMON_MODE",
        "WX_DAEMON_TASK_WORKER",
        "WX_DAEMON_OPERATION_WORKER",
        "wx_daemon_anything",
        "Wx_Cli_Expected_Runtime",
    ] {
        let mut request = valid.clone();
        request["invocation"]["environment"][name] = json!("1");
        let reply = f.call(request);
        assert_eq!(reply["ok"], false, "accepted {name}");
        assert_eq!(reply["error"]["code"], "invalid_operation");
    }
    assert_eq!(
        f.call(json!({"op":"operation_poll","id":id,"after":0}))["error"]["code"],
        "not_found"
    );
    assert_eq!(f.call(valid)["ok"], true);
    assert_eq!(f.finish(&id).code, 0);
}

fn backend(binary: Option<&Path>) -> Value {
    json!({"backend":"Local","whisper_binary":binary,"whisper_model":binary.map(|_| "model.bin"),
        "language":"zh","threads":2,"timeout_seconds":60,"allow_upload":false,
        "openai_base_url":null,"openai_model":null,"api_key_file":null,"temp_root":null})
}

fn fake_asr() -> &'static Path {
    static EXE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &EXE.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fake-asr.exe");
        let mut command = Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asr-local/fake.rs"))
            .arg("-o")
            .arg(&path)
            .creation_flags(0x08000000);
        println!("COMMAND: {command:?}");
        let output = command.output().unwrap();
        println!(
            "STDOUT:\n{}\nSTDERR:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success());
        (root, path)
    })
    .1
}

fn child_of(parent: u32) -> Option<u32> {
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap() });
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok() {
        loop {
            if entry.th32ParentProcessID == parent {
                return Some(entry.th32ProcessID);
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
                break;
            }
        }
    }
    None
}

#[test]
fn daemon_owns_asr_worker_and_reaps_worker_tree_on_cancel_and_client_lease_expiry() {
    let binary = fake_asr();
    let f = Fixture::new();
    f.boot();
    fs::write(f.root.path().join("model.bin"), "sleep").unwrap();
    fs::write(
        f.root.path().join("input.silk"),
        include_bytes!("fixtures/audio/silence.silk"),
    )
    .unwrap();
    let record = f.identity();
    let daemon = record["pid"].as_u64().unwrap() as u32;
    for cancel in [true, false] {
        let id = if cancel { "44" } else { "55" }.repeat(32);
        f.start(
            &id,
            json!({"kind":"transcribe_audio","args":{"args":{
            "input":"input.silk","backend":backend(Some(binary))}}}),
            f.root.path(),
            &f.environment(),
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        let (worker, asr) = loop {
            if let Some(worker) = child_of(daemon) {
                if let Some(asr) = child_of(worker) {
                    break (process(worker).unwrap(), process(asr).unwrap());
                }
            }
            assert!(
                Instant::now() < deadline,
                "daemon operation worker did not launch fake ASR"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        if cancel {
            f.ok(json!({"op":"operation_cancel","id":id}));
        }
        // No polls renew the lease in the disconnect case. Handles pin process identity.
        let timeout = if cancel { 5000 } else { 22000 };
        assert_eq!(
            unsafe { WaitForSingleObject(worker.0, timeout) },
            WAIT_OBJECT_0,
            "operation worker survived"
        );
        assert_eq!(
            unsafe { WaitForSingleObject(asr.0, 2000) },
            WAIT_OBJECT_0,
            "ASR descendant survived"
        );
        assert_eq!(f.finish(&id).code, 130);
        assert_eq!(f.identity(), record);
        assert_eq!(
            f.cli(
                f.root.path(),
                &["toolkit", "status", "--json"],
                &f.environment()
            )
            .code,
            0
        );
    }
}
