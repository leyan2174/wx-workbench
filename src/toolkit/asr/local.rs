//! 显式配置的 whisper.cpp CLI 后端；不下载模型、不解码 SILK。

use super::windows_supervision::{self as supervision, Caller};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct LocalConfig {
    pub executable: PathBuf,
    pub model: PathBuf,
    pub language: String,
    pub threads: usize,
    pub timeout: Duration,
    pub output_format: OutputFormat,
    /// 可选私有临时目录根；每次调用仍建立独立子目录。
    pub temp_root: Option<PathBuf>,
}

impl LocalConfig {
    pub fn new(executable: PathBuf, model: PathBuf) -> Self {
        Self {
            executable,
            model,
            language: "auto".into(),
            threads: thread::available_parallelism()
                .map_or(4, usize::from)
                .min(8),
            timeout: Duration::from_secs(120),
            output_format: OutputFormat::Text,
            temp_root: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcription {
    pub text: String,
    pub language: String,
    pub backend: String,
}

/// 运行期终止阈值；磁盘轮询存在短暂超调，不是文件系统硬配额。
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub max_temp_bytes: u64,
    pub max_response_bytes: usize,
    pub max_stream_bytes: u64,
    pub max_temp_entries: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_temp_bytes: 32 * 1024 * 1024,
            max_response_bytes: 16 * 1024 * 1024,
            max_stream_bytes: 4 * 1024 * 1024,
            max_temp_entries: 4096,
        }
    }
}

struct ProcessGuard {
    child: Child,
    #[cfg(windows)]
    job: Option<supervision::Job>,
}

impl ProcessGuard {
    fn stop(&mut self) -> Result<()> {
        supervision::stop(&mut self.child, self.job.as_ref()).context("reap whisper.cpp")
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn transcribe(config: &LocalConfig, audio: &Path) -> Result<Transcription> {
    transcribe_with_limits(config, audio, ResourceLimits::default())
}

pub fn transcribe_with_limits(
    config: &LocalConfig,
    audio: &Path,
    limits: ResourceLimits,
) -> Result<Transcription> {
    ensure!(
        limits.max_temp_bytes > 0
            && limits.max_response_bytes > 0
            && limits.max_response_bytes < usize::MAX
            && limits.max_stream_bytes > 0
            && limits.max_temp_entries > 0,
        "resource limits must be positive and bounded"
    );
    #[cfg(not(windows))]
    bail!("local ASR resource supervision requires Windows");
    ensure!(config.threads > 0, "threads must be positive");
    ensure!(!config.timeout.is_zero(), "timeout must be positive");
    ensure!(
        !config.language.is_empty()
            && config
                .language
                .bytes()
                .all(|c| c.is_ascii_alphabetic() || c == b'-'),
        "invalid language"
    );
    let executable = explicit_file(&config.executable, "whisper.cpp executable")?;
    let model = explicit_file(&config.model, "model")?;
    let audio = explicit_file(audio, "audio input")?;
    let mut builder = tempfile::Builder::new();
    builder.prefix("wx-asr-local-");
    let work = match &config.temp_root {
        Some(root) => builder.tempdir_in(root),
        None => builder.tempdir(),
    }
    .context("create ASR temporary directory")?;
    let mut owned_process = None;
    let result = (|| {
        let prefix = work.path().join("result");
        let mut command = Command::new(executable);
        command
            .arg("-m")
            .arg(model)
            .arg("-f")
            .arg(audio)
            .arg("-l")
            .arg(&config.language)
            .arg("-t")
            .arg(config.threads.to_string())
            .arg("--no-fallback")
            .arg(match config.output_format {
                OutputFormat::Text => "-otxt",
                OutputFormat::Json => "-oj",
            })
            .arg("-of")
            .arg(&prefix)
            .current_dir(work.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::windows_process::managed::SUSPENDED_NO_WINDOW);
        }
        let start = Instant::now();
        owned_process = Some(ProcessGuard {
            child: command.spawn().context("start whisper.cpp")?,
            #[cfg(windows)]
            job: None,
        });
        let process = owned_process.as_mut().expect("process created");
        #[cfg(windows)]
        {
            process.job = Some(supervision::Job::attach(
                &process.child,
                Caller::WhisperCpp,
            )?);
        }
        let mut stdout = process.child.stdout.take().context("missing stdout pipe")?;
        let mut stderr = process.child.stderr.take().context("missing stderr pipe")?;
        let mut stream_bytes = 0u64;
        let extension = match config.output_format {
            OutputFormat::Text => "txt",
            OutputFormat::Json => "json",
        };
        let result_path = prefix.with_extension(extension);
        let status = loop {
            supervision::drain(
                &mut stdout,
                &mut stream_bytes,
                limits.max_stream_bytes,
                Caller::WhisperCpp,
            )?;
            supervision::drain(
                &mut stderr,
                &mut stream_bytes,
                limits.max_stream_bytes,
                Caller::WhisperCpp,
            )?;
            check_disk(work.path(), &result_path, limits, start, config.timeout)?;
            if let Some(status) = process.child.try_wait().context("poll whisper.cpp")? {
                break status;
            }
            if start.elapsed() >= config.timeout {
                bail!("whisper.cpp timed out after {:?}", config.timeout);
            }
            thread::sleep(
                Duration::from_millis(10).min(config.timeout.saturating_sub(start.elapsed())),
            );
        };
        // 主进程正常退出也要收回后台子进程，关闭其继承的管道和文件。
        process.stop()?;
        supervision::drain(
            &mut stdout,
            &mut stream_bytes,
            limits.max_stream_bytes,
            Caller::WhisperCpp,
        )?;
        supervision::drain(
            &mut stderr,
            &mut stream_bytes,
            limits.max_stream_bytes,
            Caller::WhisperCpp,
        )?;
        check_disk(work.path(), &result_path, limits, start, config.timeout)?;
        if !status.success() {
            bail!("whisper.cpp failed ({status}); process output withheld");
        }
        let file =
            File::open(result_path).context("whisper.cpp did not produce requested output")?;
        let mut bytes = Vec::new();
        file.take(limits.max_response_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limits.max_response_bytes,
            "ASR response limit exceeded"
        );
        parse_output(
            std::str::from_utf8(&bytes).context("ASR output is not UTF-8")?,
            config.output_format,
            &config.language,
        )
    })();
    let cleanup = owned_process.as_mut().map_or(Ok(()), ProcessGuard::stop);
    drop(owned_process);
    cleanup.context("Unable to confirm ASR child cleanup")?;
    // Job 计数归零后，Windows 仍可能短暂持有已终止进程的文件句柄。
    // 在进程和管道 RAII 完成之后有界重试；清理失败不能静默报告成功。
    let cleanup_start = Instant::now();
    loop {
        match fs::remove_dir_all(work.path()) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) if cleanup_start.elapsed() < Duration::from_secs(1) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => bail!("ASR temporary directory cleanup failed"),
        }
    }
    result
}

fn check_disk(
    root: &Path,
    result: &Path,
    limits: ResourceLimits,
    start: Instant,
    timeout: Duration,
) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut size = 0u64;
    let mut entries = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            ensure!(start.elapsed() < timeout, "whisper.cpp timed out");
            let entry = entry?;
            entries += 1;
            ensure!(
                entries <= limits.max_temp_entries,
                "ASR temporary entry limit exceeded"
            );
            let metadata = fs::symlink_metadata(entry.path())?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "ASR temporary reparse point rejected"
                );
            }
            ensure!(
                !metadata.file_type().is_symlink(),
                "ASR temporary link rejected"
            );
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(metadata.is_file(), "ASR temporary special file rejected");
                size = size.saturating_add(metadata.len());
                ensure!(
                    size <= limits.max_temp_bytes,
                    "ASR temporary disk limit exceeded"
                );
                if entry.path() == result {
                    ensure!(
                        metadata.len() <= limits.max_response_bytes as u64,
                        "ASR response limit exceeded"
                    );
                }
            }
        }
    }
    Ok(())
}

fn explicit_file(path: &Path, label: &str) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty(),
        "{label} must be explicitly configured"
    );
    ensure!(path.is_file(), "{label} is not a file: {}", path.display());
    fs::canonicalize(path).with_context(|| format!("resolve {label}"))
}

/// 只解析指定结果文件，不将 CLI 日志当成识别文本。
pub fn parse_output(
    output: &str,
    format: OutputFormat,
    requested_language: &str,
) -> Result<Transcription> {
    let output = output.trim_start_matches('\u{feff}');
    let fallback = if requested_language == "auto" {
        "unknown"
    } else {
        requested_language
    };
    let (text, language) = match format {
        OutputFormat::Text => (output.trim().to_owned(), fallback.to_owned()),
        OutputFormat::Json => {
            let value: serde_json::Value =
                serde_json::from_str(output).context("invalid whisper.cpp JSON")?;
            let text = if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
                text.to_owned()
            } else if let Some(segments) = value.get("transcription").and_then(|v| v.as_array()) {
                let mut text = String::new();
                for segment in segments {
                    text.push_str(
                        segment
                            .get("text")
                            .and_then(|v| v.as_str())
                            .context("JSON segment missing text")?,
                    );
                }
                text
            } else {
                bail!("whisper.cpp JSON missing text/transcription");
            };
            let language = value
                .pointer("/result/language")
                .or_else(|| value.get("language"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty() && *s != "auto")
                .unwrap_or(fallback);
            (text.trim().to_owned(), language.to_owned())
        }
    };
    Ok(Transcription {
        text,
        language,
        backend: "whisper_cpp".into(),
    })
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
