use super::*;
use std::sync::OnceLock;

#[test]
#[cfg(windows)]
fn shared_job_can_move_with_python_worker() {
    fn require_send<T: Send>() {}
    require_send::<super::super::windows_supervision::Job>();
}

fn executable() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("fake.rs");
            fs::write(
                &source,
                include_str!("../../../tests/fixtures/asr-local/fake.rs"),
            )
            .unwrap();
            let exe = dir.path().join(if cfg!(windows) {
                "fake whisper.exe"
            } else {
                "fake whisper"
            });
            let mut command = Command::new("rustc");
            command
                .arg("--edition=2021")
                .arg(source)
                .arg("-o")
                .arg(&exe);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            (dir, exe)
        })
        .1
}

fn run(mode: &str, format: OutputFormat) -> Result<Transcription> {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model with spaces.bin");
    let audio = dir.path().join("synthetic input.wav");
    fs::write(&model, mode).unwrap();
    fs::write(&audio, b"synthetic fixture, not real audio").unwrap();
    let mut config = LocalConfig::new(executable().to_path_buf(), model);
    config.language = "zh".into();
    config.threads = 2;
    config.timeout = if mode == "sleep" {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(5)
    };
    config.output_format = format;
    config.temp_root = Some(dir.path().to_owned());
    let start = Instant::now();
    let result = transcribe(&config, &audio);
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        2,
        "temporary outputs leaked"
    );
    assert_eq!(
        fs::read(audio).unwrap(),
        b"synthetic fixture, not real audio"
    );
    result
}

#[test]
fn process_text() {
    assert_eq!(run("ok", OutputFormat::Text).unwrap().text, "hello world");
}
#[test]
fn process_json() {
    let r = run("ok", OutputFormat::Json).unwrap();
    assert_eq!(r.text, "hello world");
    assert_eq!(r.language, "zh");
}
#[test]
fn process_failure() {
    let error = run("fail", OutputFormat::Text).unwrap_err().to_string();
    assert!(error.contains("failed"));
    assert!(!error.contains("synthetic failure"));
}

fn resource_executable() -> &'static Path {
    use std::io::Write;
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("resource fixture.exe");
        let mut command = Command::new("rustc");
        command.args(["--edition=2021", "--crate-name", "asr_resource_fixture", "-"])
            .arg("-o").arg(&exe).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(br#"
use std::{env, fs, io::Write, path::PathBuf, process::Command, thread, time::Duration};
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("child") {
        let mut f = fs::File::create(&args[2]).unwrap();
        for _ in 0..1000 { f.write_all(b"x").unwrap(); f.flush().unwrap(); thread::sleep(Duration::from_millis(5)); }
        return;
    }
    let arg = |key: &str| &args[args.iter().position(|s| s == key).unwrap() + 1];
    let mode = fs::read_to_string(arg("-m")).unwrap();
    if mode.starts_with("tree") {
        thread::sleep(Duration::from_millis(50));
        let marker = PathBuf::from(arg("-f")).with_extension("heartbeat");
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.arg("child").arg(&marker);
        #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); }
        let _child = cmd.spawn().unwrap();
        while !marker.exists() { thread::sleep(Duration::from_millis(5)); }
        if mode == "tree-exit" { fs::write(PathBuf::from(arg("-of")).with_extension("txt"), "ok").unwrap(); return; }
    }
    if mode == "stream" {
        let t = thread::spawn(|| { loop { let _ = std::io::stdout().write_all(&[b's'; 8192]); } });
        for _ in 0..100000 { let _ = std::io::stderr().write_all(b"SYNTHETIC_SECRET_AUDIO_MODEL_CREDENTIAL"); }
        let _ = t.join();
    }
    if mode == "response" { fs::write(PathBuf::from(arg("-of")).with_extension("txt"), vec![b'x'; 131072]).unwrap(); }
    if mode == "disk" || mode == "tree-disk" {
        for n in 0..10000 { fs::write(format!("extra-{n}.bin"), vec![0; 16384]).unwrap(); thread::sleep(Duration::from_millis(1)); }
    }
    if mode == "entries" { for n in 0..10000 { fs::write(format!("entry-{n}"), []).unwrap(); } }
    thread::sleep(Duration::from_secs(10));
}
"#).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        (dir, exe)
    }).1
}

fn resource_run(mode: &str, limits: ResourceLimits) -> Result<Transcription> {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model");
    let audio = dir.path().join("synthetic.wav");
    fs::write(&model, mode).unwrap();
    fs::write(&audio, "synthetic").unwrap();
    let mut config = LocalConfig::new(resource_executable().to_owned(), model);
    config.temp_root = Some(dir.path().to_owned());
    config.timeout = Duration::from_millis(800);
    let start = Instant::now();
    let result = transcribe_with_limits(&config, &audio, limits);
    assert!(start.elapsed() < Duration::from_secs(4));
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("wx-asr-local-")));
    if mode.starts_with("tree") {
        let marker = audio.with_extension("heartbeat");
        let before = fs::metadata(&marker).expect("child actually started").len();
        assert!(before > 0);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            fs::metadata(marker).unwrap().len(),
            before,
            "descendant still writing"
        );
    }
    if let Err(error) = &result {
        assert!(!format!("{error:#}").contains("SYNTHETIC_SECRET"));
    }
    result
}

#[test]
fn temporary_disk_growth_is_stopped() {
    let limits = ResourceLimits {
        max_temp_bytes: 65536,
        ..ResourceLimits::default()
    };
    let error = resource_run("disk", limits).unwrap_err();
    assert!(error.to_string().contains("disk limit"), "{error:#}");
}

#[test]
fn response_growth_is_stopped() {
    let limits = ResourceLimits {
        max_response_bytes: 1024,
        ..ResourceLimits::default()
    };
    assert!(resource_run("response", limits)
        .unwrap_err()
        .to_string()
        .contains("response limit"));
}

#[test]
fn both_output_streams_are_bounded_and_private() {
    let limits = ResourceLimits {
        max_stream_bytes: 4096,
        ..ResourceLimits::default()
    };
    assert!(resource_run("stream", limits)
        .unwrap_err()
        .to_string()
        .contains("stream limit"));
}

#[test]
fn temporary_entry_count_is_bounded() {
    let limits = ResourceLimits {
        max_temp_entries: 8,
        ..ResourceLimits::default()
    };
    assert!(resource_run("entries", limits)
        .unwrap_err()
        .to_string()
        .contains("entry limit"));
}

#[test]
fn job_reaps_descendants_on_timeout() {
    assert!(resource_run("tree-hang", ResourceLimits::default())
        .unwrap_err()
        .to_string()
        .contains("timed out"));
}

#[test]
fn job_reaps_descendants_on_disk_limit() {
    let limits = ResourceLimits {
        max_temp_bytes: 65536,
        ..ResourceLimits::default()
    };
    let error = resource_run("tree-disk", limits).unwrap_err();
    assert!(error.to_string().contains("disk limit"), "{error:#}");
}

#[test]
fn job_reaps_descendants_after_parent_exits() {
    assert_eq!(
        resource_run("tree-exit", ResourceLimits::default())
            .unwrap()
            .text,
        "ok"
    );
}
#[test]
fn process_timeout_cleanup() {
    assert!(run("sleep", OutputFormat::Text)
        .unwrap_err()
        .to_string()
        .contains("timed out"));
}
#[test]
fn process_missing_output() {
    assert!(run("missing", OutputFormat::Text)
        .unwrap_err()
        .to_string()
        .contains("did not produce"));
}
#[test]
fn process_invalid_json() {
    assert!(run("malformed", OutputFormat::Json).is_err());
}
#[test]
fn parsing() {
    let r = parse_output("\u{feff} 你好 \n", OutputFormat::Text, "auto").unwrap();
    assert_eq!(r.text, "你好");
    assert_eq!(r.language, "unknown");
    assert_eq!(
        parse_output(
            r#"{"text":" hello ","language":"en"}"#,
            OutputFormat::Json,
            "auto"
        )
        .unwrap()
        .language,
        "en"
    );
    assert!(parse_output("{}", OutputFormat::Json, "zh").is_err());
    assert!(parse_output(r#"{"transcription":[{}]}"#, OutputFormat::Json, "zh").is_err());
    assert_eq!(
        parse_output(r#"{"transcription":[]}"#, OutputFormat::Json, "zh")
            .unwrap()
            .text,
        ""
    );
}
#[test]
fn invalid_config() {
    let mut config = LocalConfig::new(PathBuf::new(), PathBuf::new());
    assert!(transcribe(&config, Path::new("missing")).is_err());
    config.threads = 0;
    assert!(transcribe(&config, Path::new("missing"))
        .unwrap_err()
        .to_string()
        .contains("threads"));
}
