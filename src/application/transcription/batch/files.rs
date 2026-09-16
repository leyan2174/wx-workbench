//! 独立命令的恢复合并及原子发布；消息填充仍复用 writeback::transcribe_json。
use super::{BatchTranscriber, Report};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(super) fn process_file(
    owner: &mut BatchTranscriber,
    input: &Path,
    output: &Path,
) -> Result<Report> {
    for path in [input, output] {
        ensure!(
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
            "only JSON export paths are accepted"
        );
    }
    owner.protect_output(output)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output = parent
        .canonicalize()?
        .join(output.file_name().context("output filename required")?);
    let _lock = PublicationLock::acquire(&output)?;
    let before = OutputSnapshot::capture(&output)?;
    let mut document: Value =
        serde_json::from_slice(&fs::read(input).context("read input chat JSON")?)?;
    validate_account(&document, &owner.runtime.id)?;
    if let Some(snapshot) = &before {
        let previous: Value =
            serde_json::from_slice(&snapshot.bytes).context("existing output is not chat JSON")?;
        validate_account(&previous, &owner.runtime.id)?;
        merge_transcriptions(&mut document, &previous)?;
    }
    let report = owner.process(&mut document)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".wx-asr-batch-")
        .tempfile_in(output.parent().expect("canonical parent"))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &document)?;
    temporary.as_file_mut().write_all(b"\n")?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    let current = OutputSnapshot::capture(&output)?;
    match (&before, &current) {
        (None, None) => {
            temporary
                .persist_noclobber(&output)
                .map_err(|error| error.error)
                .context("publish new transcribed chat without overwrite")?;
            return Ok(report);
        }
        (Some(before), Some(current)) => ensure!(
            before.matches(current),
            "output changed during transcription; publication refused"
        ),
        _ => anyhow::bail!(
            "output appeared or disappeared during transcription; publication refused"
        ),
    }
    // 与既有回写协议一致：协作锁贯穿读、合并和发布；非协作写者不享有严格 CAS。
    drop(current);
    drop(before);
    temporary
        .persist(&output)
        .map_err(|error| error.error)
        .context("replace transcribed chat atomically")?;
    Ok(report)
}

fn validate_account(document: &Value, account: &str) -> Result<()> {
    if let Some(value) = document.get("account_id") {
        ensure!(
            value.as_str() == Some(account),
            "chat export belongs to another account"
        );
    }
    Ok(())
}

fn username(document: &Value) -> Result<&str> {
    document
        .get("username")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .context("chat requires explicit username")
}

fn voice_identity(message: &Value, user: &str) -> Result<(String, i64)> {
    if let Some(name) = message.get("username") {
        ensure!(
            name.as_str() == Some(user),
            "message username conflicts with chat"
        );
    }
    let source = message
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| !source.trim().is_empty() && !source.eq_ignore_ascii_case("unknown"))
        .context("resume voice requires explicit source")?;
    let local_id = message
        .get("local_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .context("resume voice requires positive local_id")?;
    Ok((source.replace('\\', "/").to_ascii_lowercase(), local_id))
}

fn voice_index(messages: &[Value], user: &str) -> Result<BTreeMap<(String, i64), usize>> {
    let mut index = BTreeMap::new();
    for (position, message) in messages.iter().enumerate() {
        if message.get("type").and_then(Value::as_str) == Some("voice") {
            ensure!(
                index
                    .insert(voice_identity(message, user)?, position)
                    .is_none(),
                "duplicate voice identity makes resume ambiguous"
            );
        }
    }
    Ok(index)
}

fn merge_transcriptions(document: &mut Value, previous: &Value) -> Result<()> {
    let user = username(document)?.to_owned();
    ensure!(
        username(previous)? == user,
        "existing output belongs to another username"
    );
    let old = previous
        .get("messages")
        .and_then(Value::as_array)
        .context("previous messages must be an array")?;
    let messages = document
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .context("input messages must be an array")?;
    let old_index = voice_index(old, &user)?;
    let current_index = voice_index(messages, &user)?;
    // 先完成全量身份检查，再补字段；任何冲突不产生部分合并结果或触发推理。
    let mut additions = Vec::new();
    for (key, old_position) in old_index {
        let current_position = *current_index
            .get(&key)
            .context("previous voice identity is absent from input; resume refused")?;
        let previous = &old[old_position];
        let current = &messages[current_position];
        if let (Some(old_time), Some(current_time)) =
            (previous.get("timestamp"), current.get("timestamp"))
        {
            ensure!(
                old_time == current_time,
                "voice timestamp conflicts across input and output"
            );
        }
        if let Some(text) = previous.get("transcription") {
            if let Some(existing) = current.get("transcription") {
                ensure!(
                    text == existing,
                    "existing transcriptions conflict; refusing to overwrite either version"
                );
            } else {
                additions.push((current_position, text.clone()));
            }
        }
    }
    for (position, text) in additions {
        messages[position]["transcription"] = text;
    }
    Ok(())
}

// 与 writeback 的锁名保持一致，旧入口和新入口不能并发发布同一个输出。
struct PublicationLock {
    file: Option<File>,
    path: PathBuf,
}
impl PublicationLock {
    fn acquire(output: &Path) -> Result<Self> {
        let name = output
            .file_name()
            .context("output filename missing")?
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
            .context("output publication locked; do not remove an active lock")?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}
impl Drop for PublicationLock {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

struct OutputSnapshot {
    identity: same_file::Handle,
    bytes: Vec<u8>,
    modified: SystemTime,
}
impl OutputSnapshot {
    fn capture(path: &Path) -> Result<Option<Self>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "output must be a regular file"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "output reparse point rejected"
            );
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1 | 4);
        }
        let mut file = options.open(path)?;
        let modified = file.metadata()?.modified()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        ensure!(
            modified == file.metadata()?.modified()?,
            "output changed while being read"
        );
        Ok(Some(Self {
            identity: same_file::Handle::from_file(file)?,
            bytes,
            modified,
        }))
    }

    fn matches(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.bytes == other.bytes
            && self.modified == other.modified
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({"username":"peer", "messages":[
            {"type":"voice", "source":"message/message_0.db", "local_id":7},
            {"type":"voice", "source":"message/message_1.db", "local_id":7}
        ]})
    }

    #[test]
    fn resume_matches_source_identity_not_local_id_alone() {
        let mut current = document();
        let old = json!({"username":"peer", "messages":[
            {"type":"voice", "source":"MESSAGE\\MESSAGE_1.DB", "local_id":7,
             "transcription":"synthetic transcript"}
        ]});
        merge_transcriptions(&mut current, &old).unwrap();
        assert!(current["messages"][0].get("transcription").is_none());
        assert_eq!(
            current["messages"][1]["transcription"],
            "synthetic transcript"
        );
    }

    #[test]
    fn ambiguous_or_foreign_identity_never_partially_merges() {
        for foreign in [false, true] {
            let mut current = document();
            let before = current.clone();
            let mut old = document();
            old["messages"][0]["transcription"] = json!("must not merge");
            if foreign {
                old["username"] = json!("another-account-peer");
            } else {
                old["messages"][1]["source"] = json!("MESSAGE\\MESSAGE_0.DB");
            }
            assert!(merge_transcriptions(&mut current, &old).is_err());
            assert_eq!(current, before);
        }
    }
}
