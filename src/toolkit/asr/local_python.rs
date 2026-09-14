//! 迁移期间保留旧 Whisper/PyTorch 推理语义；仅推理仍依赖 Python。
//! 不导入 mcp_server，不读取账号配置、数据库或云端凭证，不是失败回退。
use super::windows_supervision::{self as supervision, Caller};
use super::Transcription;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const MAX_RESPONSE: u64 = 1024 * 1024;
const MAX_STREAM: u64 = 1024 * 1024;
const MAX_TEMP: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

// 启动握手先于第三方模块导入；只有加入 Job Object 后宿主才发送参数。
// 命名模型沿用 Whisper 自身下载及摘要检查，音频不上传；异常内容不越过桥。
const BRIDGE: &str = r#"
import sys, os, json, hashlib, time

def emit(value):
    data = json.dumps(value, ensure_ascii=False).encode('utf-8')
    if len(data) > 1048576:
        data = b'{"ok":false,"error":"response_limit"}'
    with open(os.path.join(work, 'response.tmp'), 'wb') as out:
        out.write(data)
    os.replace(os.path.join(work, 'response.tmp'), os.path.join(work, 'response.json'))

def file_hash(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as src:
        while True:
            block = src.read(65536)
            if not block:
                break
            digest.update(block)
    return digest.hexdigest()

if sys.stdin.buffer.read(1) != b'G':
    sys.exit(1)
settings = json.loads(sys.argv[1])
work = settings['work']
try:
    # 在 Job Object 握手后恢复普通 Python 的用户包、虚拟环境及 sitecustomize 语义。
    import site
    site.main()
    import whisper
    import torch
    import importlib.metadata
    if settings['threads'] is not None:
        torch.set_num_threads(settings['threads'])
    name = settings['model']
    models = getattr(whisper, '_MODELS', {})
    if name in models:
        selected_model = name
        model_identity = ['named', name, models[name]]
    else:
        # 相对权重只按宿主固定的根解析，不依赖导入模块可能改变的当前目录。
        selected_model = os.path.join(settings['model_root'], name)
        if not os.path.isfile(selected_model):
            raise ValueError('unsupported model')
        selected_model = os.path.realpath(selected_model)
        model_identity = ['file', selected_model, file_hash(selected_model)]
    try:
        whisper_version = importlib.metadata.version('openai-whisper')
    except importlib.metadata.PackageNotFoundError:
        whisper_version = getattr(whisper, '__version__', 'unversioned-source')
    descriptor = {
        'model': model_identity,
        'whisper': whisper_version,
        'torch': torch.__version__,
        'torch_cuda': torch.version.cuda,
        'device': 'cuda' if torch.cuda.is_available() else 'cpu',
        'device_name': torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
        'python': sys.version,
        'threads': torch.get_num_threads(),
        'language': settings['language'],
        'engine_sources': [file_hash(whisper.__file__), file_hash(sys.modules[whisper.transcribe.__module__].__file__)],
    }
    fingerprint = hashlib.sha256(json.dumps(descriptor, sort_keys=True).encode('utf-8')).hexdigest()
    emit({'ok': True, 'engine_identity': fingerprint})
except Exception:
    emit({'ok': False, 'error': 'dependency_or_model_configuration'})
    sys.exit(1)

model = None
while True:
    request_path = os.path.join(work, 'request.json')
    if not os.path.isfile(request_path):
        time.sleep(0.01)
        continue
    try:
        with open(request_path, 'rb') as source:
            request = json.loads(source.read(16385))
        os.remove(request_path)
        audio = request['audio']
        if os.path.realpath(audio) != os.path.realpath(os.path.join(work, 'audio.wav')):
            raise ValueError('invalid audio path')
    except Exception:
        emit({'ok': False, 'error': 'invalid_request'})
        continue
    if model is None:
        try:
            model = whisper.load_model(selected_model)
        except Exception:
            emit({'ok': False, 'error': 'model_load_failed'})
            continue
    try:
        if settings['language'] is None:
            result = model.transcribe(audio)
        else:
            result = model.transcribe(audio, language=settings['language'])
        emit({'ok': True, 'text': result.get('text', '').strip(),
              'language': result.get('language', 'unknown')})
    except Exception:
        emit({'ok': False, 'error': 'inference_failed'})
"#;

/// 发现结果和配置在构造时固定；不运行 Python 或下载模型。
#[derive(Clone)]
pub struct LocalPythonConfig {
    pub timeout: Duration,
    deadline: Option<Instant>,
    python: std::result::Result<PathBuf, String>,
    working_root: PathBuf,
    model: String,
    language: Option<String>,
    threads: Option<usize>,
    temp_root: PathBuf,
    environment: Vec<(OsString, OsString)>,
    worker: Arc<Mutex<Option<Worker>>>,
}

impl std::fmt::Debug for LocalPythonConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPythonConfig")
            .field("engine", &"legacy-python-local")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl LocalPythonConfig {
    pub fn discover(
        model: String,
        language: Option<String>,
        threads: Option<usize>,
        timeout: Duration,
        temp_root: PathBuf,
        config_parent: PathBuf,
    ) -> Result<Self> {
        ensure!(
            !model.trim().is_empty() && model.len() <= 4096 && !model.chars().any(char::is_control),
            "invalid local_whisper_model"
        );
        ensure!(Path::new(&model).is_absolute()
            || !matches!(Path::new(&model).components().next(), Some(std::path::Component::Prefix(_))),
            "drive-relative model paths are ambiguous; use an absolute or configuration-relative path");
        ensure!(
            !timeout.is_zero() && threads.is_none_or(|n| n > 0),
            "invalid local inference limits"
        );
        if let Some(language) = &language {
            ensure!(
                !language.is_empty()
                    && language
                        .bytes()
                        .all(|c| c.is_ascii_alphabetic() || c == b'-'),
                "invalid inference language"
            );
        }
        let working_root = inference_root(&config_parent)?;
        // 固定本次发现结果；没有待转录语音时不因机器缺少推理依赖阻断恢复。
        let python = resolve_python(crate::toolkit::legacy::toolkit_python())
            .map_err(|error| format!("{error:#}"));
        let environment = [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "COMMONPROGRAMFILES",
            "XDG_CACHE_HOME",
            "TORCH_HOME",
            "CUDA_VISIBLE_DEVICES",
            "CUDA_DEVICE_ORDER",
            "CUDA_MODULE_LOADING",
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "NUMBA_NUM_THREADS",
            "KMP_DUPLICATE_LIB_OK",
            "CUDA_PATH",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "PYTHONPATH",
            "PYTHONHOME",
            "PYTHONUSERBASE",
            "PYTHONNOUSERSITE",
            "PYTHONSAFEPATH",
            "VIRTUAL_ENV",
            "CONDA_PREFIX",
            "CONDA_DLL_SEARCH_MODIFICATION_ENABLE",
        ]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.into(), value)))
        .collect();
        Ok(Self {
            timeout,
            deadline: None,
            python,
            working_root,
            model,
            language,
            threads,
            temp_root,
            environment,
            worker: Arc::new(Mutex::new(None)),
        })
    }

    /// 宿主可在准备阶段固定输入文件；这里只列路径，不启动推理或读取文件内容。
    pub(crate) fn host_input_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self.python.as_ref().ok().cloned().into_iter().collect();
        let model = self.working_root.join(&self.model);
        if model.is_file() {
            paths.push(model);
        }
        paths
    }

    /// MCP 初始化、缓存身份和识别共享绝对截止时间，后续调用只能收紧预算。
    pub(crate) fn tighten_timeout(&mut self, remaining: Duration) -> Result<()> {
        ensure!(!remaining.is_zero(), "local inference deadline expired");
        let deadline = Instant::now()
            .checked_add(remaining)
            .context("invalid inference deadline")?;
        self.deadline = Some(
            self.deadline
                .map_or(deadline, |existing| existing.min(deadline)),
        );
        self.timeout = self.timeout.min(remaining);
        self.remaining_timeout()?;
        Ok(())
    }

    fn remaining_timeout(&self) -> Result<Duration> {
        let remaining = self.deadline.map_or(self.timeout, |deadline| {
            self.timeout
                .min(deadline.saturating_duration_since(Instant::now()))
        });
        ensure!(!remaining.is_zero(), "local inference deadline expired");
        Ok(remaining)
    }

    /// 同一个推理进程提供配置身份及后续识别；不另起模型探测脚本。
    pub(super) fn cache_identity(&self) -> Result<String> {
        self.with_worker(|worker| Ok(worker.identity.clone()))
    }

    fn with_worker<T>(&self, operation: impl FnOnce(&mut Worker) -> Result<T>) -> Result<T> {
        self.remaining_timeout()?;
        let start = Instant::now();
        let mut slot = loop {
            self.remaining_timeout()?;
            match self.worker.try_lock() {
                Ok(slot) => break slot,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    bail!("local inference worker lock poisoned")
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    ensure!(
                        start.elapsed() < self.timeout,
                        "local inference worker busy timeout"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
            }
        };
        if let Some(worker) = slot.as_mut() {
            if worker.process.child.try_wait()?.is_some() {
                if let Some(mut worker) = slot.take() {
                    worker.shutdown()?;
                }
            }
        }
        if slot.is_none() {
            *slot = Some(Worker::start(self)?);
        }
        let result = self
            .remaining_timeout()
            .and_then(|_| operation(slot.as_mut().expect("worker started")));
        if result.is_err() {
            if let Some(mut worker) = slot.take() {
                worker
                    .shutdown()
                    .context("local inference failed and cleanup failed")?;
            }
        }
        result
    }
}

fn inference_root(config_parent: &Path) -> Result<PathBuf> {
    let explicit_root = std::env::var_os("WX_WECHAT_DECRYPT_DIR");
    inference_root_with(config_parent, explicit_root.as_deref())
}

fn inference_root_with(
    config_parent: &Path,
    explicit_root: Option<&std::ffi::OsStr>,
) -> Result<PathBuf> {
    ensure!(
        config_parent.is_absolute(),
        "inference requires an absolute configuration directory"
    );
    let config_parent = config_parent
        .canonicalize()
        .context("resolve selected configuration directory")?;
    ensure!(
        config_parent.is_dir(),
        "selected configuration parent must be a directory"
    );
    // 仅保留显式旧根的相对模型兼容，不让单 exe 发布依赖开发机源码路径或脚本标记文件。
    if let Some(root) = explicit_root.filter(|root| !root.is_empty()) {
        let root = config_parent.join(PathBuf::from(root));
        if root.is_absolute() && root.is_dir() {
            return root
                .canonicalize()
                .context("resolve explicit inference working directory");
        }
    }
    Ok(config_parent)
}

fn resolve_python(path: PathBuf) -> Result<PathBuf> {
    if path.is_file() {
        return path.canonicalize().context("resolve legacy Python");
    }
    ensure!(
        path.components().count() == 1,
        "configured legacy Python is unavailable"
    );
    let search = std::env::var_os("PATH").context("legacy Python not found")?;
    for directory in std::env::split_paths(&search) {
        for name in [path.clone(), path.with_extension("exe")] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate.canonicalize().context("resolve legacy Python");
            }
        }
    }
    bail!("legacy Python interpreter unavailable; configure WX_WECHAT_DECRYPT_PYTHON")
}

pub fn transcribe_wav(config: &LocalPythonConfig, wav: &[u8]) -> Result<Transcription> {
    ensure!(
        wav.len() <= super::MAX_AUDIO_BYTES,
        "local inference WAV exceeds limit"
    );
    super::validate_wav(wav)?;
    config.with_worker(|worker| {
        worker.clear_response()?;
        let audio = worker.root().join("audio.wav");
        let mut file = File::create(&audio).context("create private inference WAV")?;
        file.write_all(wav)?;
        file.flush()?;
        drop(file);
        worker.send(&json!({"audio": audio}))?;
        let result = worker.response(config.remaining_timeout()?)?;
        fs::remove_file(&audio).context("remove private inference WAV")?;
        worker.clear_response()?;
        Ok(Transcription {
            text: result
                .get("text")
                .and_then(Value::as_str)
                .context("inference text missing")?
                .to_owned(),
            language: result
                .get("language")
                .and_then(Value::as_str)
                .context("inference language missing")?
                .to_owned(),
            backend: "legacy-python-local".into(),
        })
    })
}

struct Worker {
    process: Process,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    work: Option<tempfile::TempDir>,
    identity: String,
    // 固定可执行文件及显式权重文件，避免识别期间被替换。
    _identity_files: Vec<File>,
}

impl Worker {
    fn start(config: &LocalPythonConfig) -> Result<Self> {
        #[cfg(not(windows))]
        bail!("legacy Python inference supervision requires Windows");
        config.remaining_timeout()?;
        let python = config
            .python
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(
            config.working_root.is_dir(),
            "fixed inference working directory unavailable"
        );
        fs::create_dir_all(&config.temp_root).context("create account inference temporary root")?;
        let work = tempfile::Builder::new()
            .prefix("local-inference-")
            .tempdir_in(&config.temp_root)?;
        let (program_hash, program) = super::cached::file_digest(python)?;
        let mut files = vec![program];
        let model_file = config.working_root.join(&config.model);
        if model_file.is_file() {
            let (_, file) = super::cached::file_digest(&model_file)?;
            files.push(file);
        }
        let mut command = Command::new(python);
        let settings = serde_json::to_vec(&json!({"work": work.path(), "model": config.model,
            "model_root": config.working_root, "language": config.language, "threads": config.threads}))?;
        ensure!(
            settings.len() <= 8192,
            "inference configuration limit exceeded"
        );
        command
            .arg("-S")
            .arg("-u")
            .arg("-B")
            .arg("-c")
            .arg(BRIDGE)
            .arg(std::str::from_utf8(&settings)?)
            .current_dir(&config.working_root)
            .env_clear()
            .envs(config.environment.iter().cloned())
            .env("PYTHONUTF8", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("TMP", work.path())
            .env("TEMP", work.path())
            .env("NUMBA_CACHE_DIR", work.path().join("numba"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        config.remaining_timeout()?;
        let mut process = Process {
            child: command.spawn().context("start legacy local inference")?,
            #[cfg(windows)]
            job: None,
        };
        #[cfg(windows)]
        {
            process.job = Some(supervision::Job::attach(&process.child, Caller::Python)?);
        }
        let stdout = process
            .child
            .stdout
            .take()
            .context("inference stdout unavailable")?;
        let stderr = process
            .child
            .stderr
            .take()
            .context("inference stderr unavailable")?;
        let mut worker = Self {
            process,
            stdout,
            stderr,
            work: Some(work),
            identity: String::new(),
            _identity_files: files,
        };
        // 空管道中仅写一个握手字节；之后请求用原子文件发布，避免阻塞管道写绕过超时。
        worker
            .process
            .child
            .stdin
            .as_mut()
            .context("inference gate unavailable")?
            .write_all(b"G")?;
        worker.process.child.stdin.take();
        let ready = worker.response(config.remaining_timeout()?)?;
        let engine = ready
            .get("engine_identity")
            .and_then(Value::as_str)
            .context("inference engine identity missing")?;
        ensure!(
            engine.len() == 64 && engine.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid inference engine identity"
        );
        let identity = serde_json::to_vec(&(
            "legacy-python-local-v1",
            program_hash,
            engine,
            python.to_string_lossy(),
            config.working_root.to_string_lossy(),
            BRIDGE,
        ))?;
        worker.identity = format!("{:x}", Sha256::digest(identity));
        Ok(worker)
    }

    fn root(&self) -> &Path {
        self.work.as_ref().expect("worker directory alive").path()
    }

    fn send(&mut self, request: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(request)?;
        ensure!(bytes.len() < 16_384, "inference request limit exceeded");
        let temporary = self.root().join("request.tmp");
        fs::write(&temporary, bytes).context("write local inference request")?;
        fs::rename(temporary, self.root().join("request.json"))
            .context("publish local inference request")
    }

    fn clear_response(&self) -> Result<()> {
        match fs::remove_file(self.root().join("response.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => bail!("clear previous inference response failed"),
        }
    }

    fn response(&mut self, timeout: Duration) -> Result<Value> {
        let start = Instant::now();
        let mut stream_bytes = 0;
        loop {
            ensure!(
                start.elapsed() < timeout,
                "legacy local inference timed out"
            );
            supervision::drain(
                &mut self.stdout,
                &mut stream_bytes,
                MAX_STREAM,
                Caller::Python,
            )?;
            supervision::drain(
                &mut self.stderr,
                &mut stream_bytes,
                MAX_STREAM,
                Caller::Python,
            )?;
            check_disk(self.root(), start, timeout)?;
            let path = self.root().join("response.json");
            if path.is_file() {
                let mut bytes = Vec::new();
                File::open(path)?
                    .take(MAX_RESPONSE + 1)
                    .read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() as u64 <= MAX_RESPONSE,
                    "local inference response limit exceeded"
                );
                let response: Value = serde_json::from_slice(&bytes).map_err(|_| {
                    anyhow::anyhow!("invalid local inference response; output withheld")
                })?;
                if response.get("ok").and_then(Value::as_bool) != Some(true) {
                    let category = match response.get("error").and_then(Value::as_str) {
                        Some("dependency_or_model_configuration") => {
                            "dependency_or_model_configuration"
                        }
                        Some("model_load_failed") => "model_load_failed",
                        Some("inference_failed") => "inference_failed",
                        Some("response_limit") => "response_limit",
                        _ => "protocol_error",
                    };
                    bail!("legacy local inference failed: {category}; process output withheld");
                }
                return Ok(response);
            }
            ensure!(
                self.process.child.try_wait()?.is_none(),
                "legacy local inference exited; process output withheld"
            );
            thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(start.elapsed())));
        }
    }

    fn shutdown(&mut self) -> Result<()> {
        self.process.stop()?;
        if let Some(work) = self.work.as_ref() {
            let start = Instant::now();
            loop {
                match fs::remove_dir_all(work.path()) {
                    Ok(()) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(_) if start.elapsed() < Duration::from_secs(1) => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => bail!("local inference temporary cleanup failed"),
                }
            }
        }
        self.work.take();
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            eprintln!("[asr-batch] legacy local inference cleanup failed");
        }
    }
}

struct Process {
    child: Child,
    #[cfg(windows)]
    job: Option<supervision::Job>,
}

impl Process {
    fn stop(&mut self) -> Result<()> {
        self.child.stdin.take();
        #[cfg(windows)]
        let job_result = self.job.as_ref().map_or(Ok(()), |job| job.terminate());
        let _ = self.child.kill();
        let start = Instant::now();
        while self.child.try_wait()?.is_none() {
            ensure!(
                start.elapsed() < Duration::from_secs(2),
                "local inference reap timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        #[cfg(windows)]
        job_result?;
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn check_disk(root: &Path, start: Instant, timeout: Duration) -> Result<()> {
    let mut pending = vec![root.to_owned()];
    let mut count = 0;
    let mut bytes = 0u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            ensure!(start.elapsed() < timeout, "local inference timed out");
            count += 1;
            ensure!(
                count <= MAX_ENTRIES,
                "local inference temporary entry limit exceeded"
            );
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "local inference temporary link rejected"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "local inference reparse point rejected"
                );
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(
                    metadata.is_file(),
                    "local inference temporary special file rejected"
                );
                bytes = bytes.saturating_add(metadata.len());
                ensure!(
                    bytes <= MAX_TEMP,
                    "local inference temporary byte limit exceeded"
                );
                if entry.file_name() == "response.json" || entry.file_name() == "response.tmp" {
                    ensure!(
                        metadata.len() <= MAX_RESPONSE,
                        "local inference response limit exceeded"
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod root_tests {
    use super::*;
    use std::ffi::OsStr;

    fn synthetic_config(root: &Path) -> LocalPythonConfig {
        LocalPythonConfig {
            timeout: Duration::from_secs(120),
            deadline: None,
            python: Err("SYNTHETIC_PRIVATE_PYTHON_PATH".into()),
            working_root: root.to_owned(),
            model: "synthetic.pt".into(),
            language: None,
            threads: None,
            temp_root: root.join("private-work"),
            environment: Vec::new(),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn host_deadline_cannot_be_extended_or_revived() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = synthetic_config(directory.path());
        config.tighten_timeout(Duration::from_secs(10)).unwrap();
        let deadline = config.deadline;
        config.tighten_timeout(Duration::from_secs(120)).unwrap();
        assert_eq!(config.deadline, deadline);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.remaining_timeout().unwrap() <= Duration::from_secs(10));
        config.deadline = Some(Instant::now());
        assert!(config.tighten_timeout(Duration::from_secs(120)).is_err());
        assert!(config.remaining_timeout().is_err());
    }

    #[test]
    fn expired_host_deadline_precedes_python_start_and_private_work_creation() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = synthetic_config(directory.path());
        config.deadline = Some(Instant::now());
        let error = config.cache_identity().unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(!format!("{error:#} {config:?}").contains("SYNTHETIC_PRIVATE_PYTHON_PATH"));
        assert!(config.worker.lock().unwrap().is_none());
        assert!(!config.temp_root.exists());
    }

    #[test]
    fn host_input_paths_include_only_selected_python_and_existing_model() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = synthetic_config(directory.path());
        assert!(config.host_input_paths().is_empty());
        let python = directory.path().join("synthetic-python.exe");
        let model = directory.path().join("synthetic.pt");
        fs::write(&python, b"not executable").unwrap();
        fs::write(&model, b"not a model").unwrap();
        config.python = Ok(python.clone());
        assert_eq!(config.host_input_paths(), vec![python, model]);
        assert!(!config.temp_root.exists());
    }

    #[test]
    fn packaged_config_parent_needs_no_legacy_scripts() {
        let directory = tempfile::tempdir().unwrap();
        let root = inference_root_with(directory.path(), None).unwrap();
        assert_eq!(root, directory.path().canonicalize().unwrap());
        assert!(!root.join("mcp_server.py").exists());
        assert!(!root.join("vendor").exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        assert!(!BRIDGE.contains("mcp_server"));
    }

    #[cfg(windows)]
    #[test]
    fn worker_without_legacy_scripts_reaches_executable_validation() {
        let directory = tempfile::tempdir().unwrap();
        let config = LocalPythonConfig {
            timeout: Duration::from_secs(1),
            deadline: None,
            python: Ok(directory.path().join("missing-synthetic-python.exe")),
            working_root: inference_root_with(directory.path(), None).unwrap(),
            model: "synthetic.pt".into(),
            language: None,
            threads: None,
            temp_root: directory.path().join("private-work"),
            environment: Vec::new(),
            worker: Arc::new(Mutex::new(None)),
        };
        // 缺失可执行文件使文件摘要阶段失败，绝不会到达 Command::spawn。
        assert!(Worker::start(&config).is_err());
        assert!(config.temp_root.is_dir());
        assert_eq!(fs::read_dir(&config.temp_root).unwrap().count(), 0);
        assert!(!config.working_root.join("mcp_server.py").exists());
    }

    #[test]
    fn explicit_relative_legacy_root_and_model_use_config_parent() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("legacy");
        fs::create_dir(&legacy).unwrap();
        let root = inference_root_with(directory.path(), Some(OsStr::new("legacy"))).unwrap();
        assert_eq!(root, legacy.canonicalize().unwrap());
        assert_eq!(
            root.join("weights").join("synthetic.pt"),
            legacy
                .canonicalize()
                .unwrap()
                .join("weights")
                .join("synthetic.pt")
        );
        assert!(!root.join("mcp_server.py").exists());
    }

    #[test]
    fn explicit_absolute_root_is_preserved_and_unavailable_root_falls_back() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        assert_eq!(
            inference_root_with(directory.path(), Some(legacy.path().as_os_str())).unwrap(),
            legacy.path().canonicalize().unwrap()
        );
        for explicit in [OsStr::new(""), OsStr::new("missing-legacy-root")] {
            assert_eq!(
                inference_root_with(directory.path(), Some(explicit)).unwrap(),
                directory.path().canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn configuration_root_must_be_absolute_existing_directory() {
        assert!(inference_root_with(Path::new("relative-config"), None).is_err());
        let directory = tempfile::tempdir().unwrap();
        assert!(inference_root_with(&directory.path().join("missing"), None).is_err());
        let file = directory.path().join("not-directory");
        fs::write(&file, b"synthetic").unwrap();
        assert!(inference_root_with(&file, None).is_err());
    }
}
