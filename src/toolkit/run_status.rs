//! 旧 run status 的原生数据统计；路径跟随所选配置，不读取密钥内容。
use anyhow::{ensure, Result};
use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{
    fmt, fs,
    io::BufReader,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize)]
pub struct Usage {
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Default, Serialize)]
pub struct Progress {
    pub voices: u64,
    pub transcribed: u64,
}

#[derive(Serialize)]
pub struct KeyFile {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct Status {
    pub config_path: PathBuf,
    pub config_exists: bool,
    pub db_dir: Option<String>,
    pub key_files: Vec<KeyFile>,
    pub decrypted_dir: PathBuf,
    pub databases: Usage,
    pub message_databases: Usage,
    pub exported_dir: PathBuf,
    pub exports: Usage,
    pub progress: Progress,
    pub unreadable_transcriptions: u64,
}

pub fn inspect(config_path: &Path, exported_dir: Option<&Path>) -> Result<Status> {
    let base = config_path.parent().unwrap_or(Path::new("."));
    let config_exists = config_path.exists();
    let config: Value = if config_exists {
        serde_json::from_reader(BufReader::new(fs::File::open(config_path)?))?
    } else {
        serde_json::json!({})
    };
    ensure!(config.is_object(), "状态配置必须为 JSON 对象");
    let configured = |field: &str, fallback: &str| -> Result<PathBuf> {
        let raw = config
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or(fallback);
        ensure!(!raw.trim().is_empty(), "状态目录配置不能为空");
        Ok(base.join(raw))
    };
    let decrypted_dir = configured("decrypted_dir", "decrypted")?;
    let exported_dir = match exported_dir {
        Some(path) => std::path::absolute(path)?,
        None => base.join("exported_chats"),
    };
    let mut keys: Vec<_> = regular_files(base)?
        .into_iter()
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            name.starts_with("all_keys") && name.ends_with(".json")
        })
        .collect();
    if let Some(path) = config
        .get("keys_file")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let path = base.join(path);
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_file()) {
            let target = path.canonicalize()?;
            if !keys
                .iter()
                .any(|old| old.canonicalize().is_ok_and(|old| old == target))
            {
                keys.push(path);
            }
        }
    }
    keys.sort();
    let key_files = keys
        .into_iter()
        .map(|path| {
            Ok(KeyFile {
                bytes: fs::metadata(&path)?.len(),
                path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut databases = Usage {
        exists: decrypted_dir.is_dir(),
        ..Usage::default()
    };
    let mut message_databases = Usage {
        exists: databases.exists,
        ..Usage::default()
    };
    if decrypted_dir.exists() {
        ensure!(databases.exists, "解密路径不是目录");
        for path in super::files::collect(&decrypted_dir, "db", false)? {
            let bytes = fs::metadata(&path)?.len();
            databases.files += 1;
            databases.bytes += bytes;
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("message")
            {
                message_databases.files += 1;
                message_databases.bytes += bytes;
            }
        }
    }
    let mut exports = Usage {
        exists: exported_dir.is_dir(),
        ..Usage::default()
    };
    let mut progress = Progress::default();
    let mut unreadable_transcriptions = 0;
    for path in regular_files(&exported_dir)? {
        let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
        if !name.ends_with(".json") {
            continue;
        }
        if name.ends_with("_transcribed.json") {
            match read_progress(&path) {
                Ok(found) => {
                    progress.voices += found.voices;
                    progress.transcribed += found.transcribed;
                }
                Err(_) => unreadable_transcriptions += 1,
            }
        } else {
            exports.files += 1;
            exports.bytes += fs::metadata(path)?.len();
        }
    }
    Ok(Status {
        config_path: config_path.to_owned(),
        config_exists,
        db_dir: config
            .get("db_dir")
            .and_then(Value::as_str)
            .map(str::to_owned),
        key_files,
        decrypted_dir,
        databases,
        message_databases,
        exported_dir,
        exports,
        progress,
        unreadable_transcriptions,
    })
}

fn regular_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    ensure!(root.is_dir(), "状态扫描路径不是目录");
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Deserialize)]
struct Message {
    #[serde(default, rename = "type")]
    kind: Value,
    #[serde(default)]
    transcription: Value,
}

#[derive(Default, Deserialize)]
struct Chat {
    #[serde(default, deserialize_with = "messages")]
    messages: Progress,
}

#[derive(Deserialize)]
struct Export {
    #[serde(default, deserialize_with = "messages")]
    messages: Progress,
    #[serde(default, deserialize_with = "chats")]
    chats: Option<Progress>,
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn messages<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Progress, D::Error> {
    struct Count;
    impl<'de> Visitor<'de> for Count {
        type Value = Progress;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("消息数组")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Progress, A::Error> {
            let mut count = Progress::default();
            while let Some(message) = seq.next_element::<Message>()? {
                if message.kind == "voice" {
                    count.voices += 1;
                    count.transcribed += u64::from(truthy(&message.transcription));
                }
            }
            Ok(count)
        }
    }
    deserializer.deserialize_seq(Count)
}

fn chats<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Progress>, D::Error> {
    struct Count;
    impl<'de> Visitor<'de> for Count {
        type Value = Option<Progress>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("会话数组")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut count = Progress::default();
            while let Some(chat) = seq.next_element::<Chat>()? {
                count.voices += chat.messages.voices;
                count.transcribed += chat.messages.transcribed;
            }
            Ok(Some(count))
        }
    }
    deserializer.deserialize_seq(Count)
}

fn read_progress(path: &Path) -> Result<Progress> {
    // 未声明的正文等字段由 Serde 跳过；只在内存保留当前消息的两个统计字段。
    let export: Export = serde_json::from_reader(BufReader::new(fs::File::open(path)?))?;
    Ok(export.chats.unwrap_or(export.messages))
}

impl Status {
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut text = String::new();
        if self.config_exists {
            writeln!(
                text,
                "[config] {}\n         db_dir = {}",
                self.config_path.display(),
                self.db_dir.as_deref().unwrap_or("?")
            )
            .unwrap();
        } else {
            writeln!(text, "[config] 未找到 {}", self.config_path.display()).unwrap();
        }
        writeln!(text, "[keys]   {} 个密钥文件", self.key_files.len()).unwrap();
        for file in &self.key_files {
            writeln!(
                text,
                "         {} ({:.0} KB)",
                file.path.display(),
                file.bytes as f64 / 1024.0
            )
            .unwrap();
        }
        if self.databases.exists {
            writeln!(
                text,
                "[decrypt] {} 个数据库 ({:.0} MB)",
                self.databases.files,
                self.databases.bytes as f64 / 1048576.0
            )
            .unwrap();
            if self.message_databases.files > 0 {
                writeln!(
                    text,
                    "          消息库: {} 个 ({:.0} MB)",
                    self.message_databases.files,
                    self.message_databases.bytes as f64 / 1048576.0
                )
                .unwrap();
            }
        } else {
            writeln!(text, "[decrypt] 未解密").unwrap();
        }
        if self.exports.exists {
            writeln!(
                text,
                "[export]  {} 个 JSON ({:.0} MB)",
                self.exports.files,
                self.exports.bytes as f64 / 1048576.0
            )
            .unwrap();
        } else {
            writeln!(text, "[export]  未导出").unwrap();
        }
        if self.progress.voices > 0 {
            writeln!(
                text,
                "[transcribe] {}/{} ({}%) 条语音已转录",
                self.progress.transcribed,
                self.progress.voices,
                self.progress.transcribed as u128 * 100 / self.progress.voices as u128
            )
            .unwrap();
        }
        if self.unreadable_transcriptions > 0 {
            writeln!(
                text,
                "[warning] {} 个转录文件无法统计，未计入进度",
                self.unreadable_transcriptions
            )
            .unwrap();
        }
        if !self.databases.exists {
            text.push_str("\n建议的下一步: wx toolkit run decrypt\n");
        } else if !self.exports.exists {
            text.push_str("\n建议的下一步: wx toolkit run export\n");
        } else {
            text.push_str("\n目录已就绪；不代表导出或转录内容已完整核验。\n");
        }
        text
    }
}

#[cfg(test)]
#[path = "run_status_tests.rs"]
mod tests;
