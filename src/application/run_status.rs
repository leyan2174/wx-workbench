//! Run-status maintenance diagnostics; paths follow the selected configuration and secrets stay unread.
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize)]
pub struct Usage {
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
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
    for field in ["keys_file", "key_store"] {
        if let Some(path) = config
            .get(field)
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
        for path in crate::infrastructure::publication::collect(&decrypted_dir, "db", false)? {
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
    for path in regular_files(&exported_dir)? {
        let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
        if !name.ends_with(".json") {
            continue;
        }
        exports.files += 1;
        exports.bytes += fs::metadata(path)?.len();
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
        if !self.databases.exists {
            text.push_str("\n建议的下一步: wx database decrypt\n");
        } else if !self.exports.exists {
            text.push_str("\n建议的下一步: wx chats export-all\n");
        } else {
            text.push_str("\n目录已就绪；不代表导出内容已完整核验。\n");
        }
        text
    }
}

#[cfg(test)]
#[path = "run_status_tests.rs"]
mod tests;
