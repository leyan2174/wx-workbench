//! 聊天 JSON 转录编排；后端与数据库访问全部由调用方负责。

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceIdentity {
    pub username: String,
    pub source: String,
    pub local_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageError {
    pub index: usize,
    pub error: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WritebackReport {
    pub transcribed: usize,
    pub skipped_existing: usize,
    pub skipped_non_voice: usize,
    pub failed: usize,
    pub errors: Vec<MessageError>,
}

fn username(data: &Value) -> Result<&str> {
    ensure!(data.is_object(), "chat must be an object");
    ensure!(
        data.get("messages").is_some_and(Value::is_array),
        "messages must be an array"
    );
    data.get("username")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .context("chat requires an explicit username")
}

fn identity(user: &str, message: &Value) -> Result<VoiceIdentity> {
    ensure!(
        message.get("type").and_then(Value::as_str) == Some("voice"),
        "not a voice message"
    );
    if let Some(value) = message.get("username") {
        ensure!(
            value.as_str() == Some(user),
            "message username conflicts with chat"
        );
    }
    let source = message
        .get("source")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .context("voice requires source")?;
    let local_id = message
        .get("local_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .context("voice requires a positive signed 64-bit local_id")?;
    Ok(VoiceIdentity {
        username: user.into(),
        source: source.into(),
        local_id,
    })
}

/// 已有字段（包括 null 和空字符串）不覆盖；逐条失败不写入错误占位文本。
pub fn transcribe_json<F>(data: &mut Value, mut transcribe: F) -> Result<WritebackReport>
where
    F: FnMut(&VoiceIdentity) -> Result<String>,
{
    let user = username(data)?.to_owned();
    let messages = data["messages"].as_array_mut().expect("validated messages");
    let mut report = WritebackReport::default();
    for (index, message) in messages.iter_mut().enumerate() {
        if message.get("type").and_then(Value::as_str) != Some("voice") {
            report.skipped_non_voice += 1;
            continue;
        }
        if message.get("transcription").is_some() {
            report.skipped_existing += 1;
            continue;
        }
        let result = identity(&user, message).and_then(|id| transcribe(&id));
        match result {
            Ok(text) if !text.trim().is_empty() => {
                message["transcription"] = Value::String(text);
                report.transcribed += 1;
            }
            result => {
                report.failed += 1;
                report.errors.push(MessageError {
                    index,
                    error: match result {
                        Err(error) => error.to_string(),
                        Ok(_) => "transcription is empty".into(),
                    },
                });
            }
        }
    }
    Ok(report)
}

fn json_path(path: &Path) -> Result<()> {
    ensure!(
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
        "only .json export paths are accepted"
    );
    Ok(())
}

/// 同路径表示原地更新导出文件；异路径不修改输入，不读取或合并旧输出。
/// 现有输出必须是同 username 的聊天 JSON，避免覆盖源库或其他会话。
pub fn transcribe_file<F>(input: &Path, output: &Path, transcribe: F) -> Result<WritebackReport>
where
    F: FnMut(&VoiceIdentity) -> Result<String>,
{
    json_path(input)?;
    json_path(output)?;
    // 固定父目录后再取锁，避免相对路径与大小写绕过同一输出的协作锁。
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output =
        fs::canonicalize(parent)?.join(output.file_name().context("output has no filename")?);
    let _publication_lock = PublicationLock::acquire(&output)?;
    let snapshot = OutputSnapshot::capture(&output, false)?;
    let mut data: Value = serde_json::from_slice(&fs::read(input).context("read chat JSON")?)
        .context("parse chat JSON")?;
    let user = username(&data)?;
    if let Some(snapshot) = &snapshot {
        let old: Value =
            serde_json::from_slice(&snapshot.bytes).context("existing output is not chat JSON")?;
        ensure!(
            username(&old)? == user,
            "output belongs to another username"
        );
    }
    let report = transcribe_json(&mut data, transcribe)?;
    atomic_write_checked(&output, snapshot, |file| {
        serde_json::to_writer_pretty(&mut *file, &data)?;
        file.write_all(b"\n")?;
        Ok(())
    })?;
    Ok(report)
}

// 锁文件仅协调采用本协议的写者；崩溃残留锁保守拒绝，不能自动偷锁。
struct PublicationLock {
    file: Option<fs::File>,
    path: PathBuf,
}
impl PublicationLock {
    fn acquire(output: &Path) -> Result<Self> {
        let name = output
            .file_name()
            .context("missing output name")?
            .to_string_lossy();
        let path = output.with_file_name(format!(".{}.wx-asr.lock", name.to_lowercase()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let file = options
            .open(&path)
            .context("output publication is locked; do not remove an active lock")?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}
impl Drop for PublicationLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

struct OutputSnapshot {
    // 保留原句柄，防止被删文件的身份被重用；初始句柄不阻止 callback 外部写入。
    identity: same_file::Handle,
    bytes: Vec<u8>,
    modified: std::time::SystemTime,
}
impl OutputSnapshot {
    fn capture(path: &Path, publishing: bool) -> Result<Option<Self>> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "output must be a regular file, not a link"
                );
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        metadata.file_attributes() & 0x400 == 0,
                        "output reparse point rejected"
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 发布检查期间禁止普通写入；DELETE 共享用于随后原子替换。
            if publishing {
                options.share_mode(1 | 4);
            }
        }
        #[cfg(not(windows))]
        let _ = publishing;
        let mut file = options.open(path).context("open output snapshot")?;
        let before = file.metadata()?.modified()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let after = file.metadata()?.modified()?;
        ensure!(before == after, "output changed while snapshotting");
        Ok(Some(Self {
            identity: same_file::Handle::from_file(file)?,
            bytes,
            modified: after,
        }))
    }
    fn matches(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.bytes == other.bytes
            && self.modified == other.modified
    }
}

// 临时文件必须与目标同目录；任何写入或替换错误均不先删除旧输出。
#[cfg(test)]
fn atomic_write<F>(output: &Path, write: F) -> Result<()>
where
    F: FnOnce(&mut fs::File) -> Result<()>,
{
    let snapshot = OutputSnapshot::capture(output, false)?;
    atomic_write_checked(output, snapshot, write)
}

fn atomic_write_checked<F>(output: &Path, snapshot: Option<OutputSnapshot>, write: F) -> Result<()>
where
    F: FnOnce(&mut fs::File) -> Result<()>,
{
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".wx-asr-")
        .tempfile_in(parent)?;
    write(temp.as_file_mut())?;
    temp.as_file_mut().flush()?;
    temp.as_file().sync_all()?;
    let current = OutputSnapshot::capture(output, true)?;
    match (&snapshot, &current) {
        (None, None) => {
            temp.persist_noclobber(output)
                .map_err(|e| e.error)
                .context("publish new chat JSON without overwrite")?;
        }
        (Some(before), Some(now)) => {
            ensure!(
                before.matches(now),
                "output changed during transcription; publication refused"
            );
            // MoveFileExW 替换前释放两份目标快照句柄，避免 Windows 拒绝访问。
            // 协作锁仍保持；此处到 persist 之间不提供非协作写者的严格 CAS。
            drop(current);
            drop(snapshot);
            temp.persist(output)
                .map_err(|e| e.error)
                .context("atomically replace chat JSON")?;
        }
        _ => anyhow::bail!(
            "output appeared or disappeared during transcription; publication refused"
        ),
    }
    Ok(())
}

#[cfg(test)]
#[path = "writeback_tests.rs"]
mod tests;
