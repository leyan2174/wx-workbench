//! 已解析聊天的 delta 导出；不读取数据库、密钥或已有完整导出。
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{FixedOffset, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RawContent {
    Null,
    Text(String),
    Bytes(Vec<u8>),
    /// 非文本 SQLite 值可由解析层传入 Python str(value) 的精确结果。
    PythonRepr(String),
}

impl RawContent {
    fn uid_text(&self) -> Option<String> {
        match self {
            Self::Null => None,
            Self::Text(s) | Self::PythonRepr(s) => Some(s.clone()),
            Self::Bytes(bytes) => {
                let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
                    b'"'
                } else {
                    b'\''
                };
                let mut s = format!("b{}", quote as char);
                for &b in bytes {
                    match b {
                        b'\t' => s.push_str("\\t"),
                        b'\n' => s.push_str("\\n"),
                        b'\r' => s.push_str("\\r"),
                        b'\\' => s.push_str("\\\\"),
                        b if b == quote => {
                            s.push('\\');
                            s.push(b as char);
                        }
                        32..=126 => s.push(b as char),
                        _ => s.push_str(&format!("\\x{b:02x}")),
                    }
                }
                s.push(quote as char);
                Some(s)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaMessage {
    pub db_path: String,
    pub local_id: i64,
    pub timestamp: i64,
    pub sender: String,
    pub msg_type: String,
    pub raw_content: RawContent,
    pub rendered: Option<Value>,
    #[serde(default)]
    pub extras: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContactMetadata {
    pub contact_remark: String,
    pub contact_nick_name: String,
    pub contact_tags: Vec<String>,
    pub contact_memo: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaChat {
    pub username: String,
    pub display_name: String,
    pub is_group: bool,
    #[serde(default)]
    pub contact: ContactMetadata,
    pub messages: Vec<DeltaMessage>,
    /// 查询/解析层失败时，不允许发布部分会话。
    #[serde(default)]
    pub source_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaWindow {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub run_id: String,
    /// 与旧进程本地时区对应；显式传入可避免测试依赖机器时区。
    pub utc_offset_seconds: i32,
    pub generated_at: String,
}

impl DeltaWindow {
    pub fn validate(&self) -> Result<()> {
        if self.start.is_none() {
            bail!("delta export requires start_ts");
        }
        safe_component(&self.run_id)?;
        if FixedOffset::east_opt(self.utc_offset_seconds).is_none() {
            bail!("invalid UTC offset");
        }
        self.date(self.start.unwrap())?;
        if let Some(end) = self.end {
            self.date(end)?;
        }
        Ok(())
    }

    fn date(&self, ts: i64) -> Result<String> {
        if ts == 0 {
            return Ok(String::new());
        }
        let offset =
            FixedOffset::east_opt(self.utc_offset_seconds).context("invalid UTC offset")?;
        let date = offset
            .timestamp_opt(ts, 0)
            .single()
            .context("timestamp out of range")?;
        Ok(date.format("%Y-%m-%d %H:%M:%S").to_string())
    }

    fn range(&self) -> Result<Value> {
        Ok(
            json!({"start": self.date(self.start.context("delta export requires start_ts")?)?,
            "end": self.end.map(|ts| self.date(ts)).transpose()?.unwrap_or_default()}),
        )
    }
}

pub fn delta_msg_uid(
    username: &str,
    db_path: &str,
    local_id: i64,
    timestamp: i64,
    msg_type: &str,
    content: &RawContent,
) -> String {
    // Windows os.path.basename 同时识别两种分隔符及盘符相对路径。
    let db = db_path.rsplit(['\\', '/']).next().unwrap_or("");
    let db = if db.as_bytes().get(1) == Some(&b':') {
        &db[2..]
    } else {
        db
    };
    let hash = content
        .uid_text()
        .map(|s| format!("{:x}", Sha256::digest(s.as_bytes())))
        .unwrap_or_default();
    let kind = if msg_type.is_empty() {
        "text"
    } else {
        msg_type
    };
    format!(
        "{:x}",
        Sha256::digest(format!("{username}|{db}|{local_id}|{timestamp}|{kind}|{hash}").as_bytes())
    )
}

pub fn delta_filename(display: &str, is_group: bool, username: &str) -> String {
    fn clean(s: &str) -> String {
        let replaced: String = s
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        let trimmed = replaced
            .trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
        if trimmed.is_empty() {
            "unknown".into()
        } else {
            trimmed.into()
        }
    }
    let label = if display.is_empty() {
        username
    } else {
        display
    };
    format!(
        "{}_{}__{}.delta.json",
        if is_group { "group" } else { "single" },
        clean(label),
        clean(username)
    )
}

pub struct PreparedDelta {
    pub result: Value,
    pub document: Option<Value>,
}

/// 纯模型：稳定排序、闭区间筛选，保留重复消息及 extras 的旧覆盖顺序。
pub fn prepare_delta(chat: &DeltaChat, window: &DeltaWindow) -> Result<PreparedDelta> {
    window.validate()?;
    if let Some(reason) = &chat.source_error {
        return Ok(PreparedDelta {
            result: failure(&chat.username, reason),
            document: None,
        });
    }
    let mut rows: Vec<_> = chat
        .messages
        .iter()
        .filter(|m| {
            m.timestamp >= window.start.unwrap()
                && window.end.map_or(true, |end| m.timestamp <= end)
        })
        .collect();
    rows.sort_by_key(|m| m.timestamp);
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let effective = row
            .extras
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&row.msg_type);
        let mut msg = json!({"msg_uid": delta_msg_uid(&chat.username, &row.db_path, row.local_id,
            row.timestamp, effective, &row.raw_content), "local_id": row.local_id,
            "timestamp": row.timestamp, "sender": row.sender});
        if effective != "text" {
            msg["type"] = json!(effective);
        }
        if let Some(content) = &row.rendered {
            if !content.is_null() {
                msg["content"] = content.clone();
            }
        }
        for (key, value) in &row.extras {
            if key != "type" {
                msg[key] = value.clone();
            }
        }
        messages.push(msg);
    }
    if messages.is_empty() {
        return Ok(PreparedDelta {
            result: json!({"success": true, "skipped": true,
            "username": chat.username, "chat": chat.display_name, "message_count": 0,
            "reason": "no messages in delta window"}),
            document: None,
        });
    }
    let date = |msg: &Value| -> Result<String> {
        match msg.get("timestamp") {
            None | Some(Value::Null) => Ok(String::new()),
            Some(v) => window.date(v.as_i64().context("invalid timestamp in extras")?),
        }
    };
    let mut output = json!({"schema_version": 1, "export_kind": "wechat_delta",
        "chat": chat.display_name, "username": chat.username, "exported_at": window.generated_at,
        "range": window.range()?, "date_first_msg": date(&messages[0])?,
        "date_last_msg": date(messages.last().unwrap())?, "message_count": messages.len(), "messages": messages});
    if chat.is_group {
        output["is_group"] = json!(true);
    } else {
        output.as_object_mut().unwrap().extend(
            serde_json::to_value(&chat.contact)?
                .as_object()
                .unwrap()
                .clone(),
        );
    }
    Ok(PreparedDelta {
        result: json!({"success": true, "username": chat.username,
        "chat": chat.display_name, "path": format!("chats/{}", delta_filename(&chat.display_name, chat.is_group, &chat.username)),
        "message_count": output["message_count"]}),
        document: Some(output),
    })
}

fn failure(username: &str, reason: &str) -> Value {
    json!({"success": false, "username": username, "message_count": 0, "reason": reason})
}

pub fn delta_manifest(
    window: &DeltaWindow,
    chats_checked: usize,
    results: &[Value],
) -> Result<Value> {
    window.validate()?;
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        if result["success"] == true && result["message_count"].as_u64().unwrap_or(0) > 0 {
            files.push(json!({"username": result["username"], "chat": result.get("chat").unwrap_or(&result["username"]),
                "path": result["path"], "message_count": result["message_count"]}));
        } else if result["success"] != true {
            errors.push(
                json!({"username": result.get("username").cloned().unwrap_or(json!("")),
                "reason": result.get("reason").cloned().unwrap_or(json!("unknown"))}),
            );
        }
    }
    Ok(
        json!({"schema_version": 1, "export_kind": "wechat_delta_run", "run_id": window.run_id,
        "generated_at": window.generated_at, "range": window.range()?, "chats_checked": chats_checked,
        "chats_with_messages": files.len(), "messages_exported": files.iter().map(|f| f["message_count"].as_u64().unwrap_or(0)).sum::<u64>(),
        "files": files, "errors": errors}),
    )
}

fn safe_component(s: &str) -> Result<()> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.ends_with(['.', ' '])
        || s.chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
    {
        bail!("unsafe path component");
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(
                &stem[3..],
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
    {
        bail!("reserved Windows path component");
    }
    Ok(())
}

fn check_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("not a plain directory: {}", path.display());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("reparse directory rejected");
        }
    }
    Ok(())
}

fn check_directory_ancestors(path: &Path) -> Result<()> {
    // 从盘符根到叶目录逐层检查，不先穿过未经检查的 junction 查询子目录。
    let ancestors: Vec<_> = path.ancestors().collect();
    for ancestor in ancestors.into_iter().rev() {
        check_directory(ancestor)?;
    }
    Ok(())
}

fn validate_root_path(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        bail!("delta root must be absolute");
    }
    root.parent().context("root requires parent")?;
    for component in root.components() {
        match component {
            Component::ParentDir | Component::CurDir => bail!("root traversal rejected"),
            Component::Normal(s) => safe_component(s.to_str().context("non-Unicode path")?)?,
            _ => {}
        }
    }
    Ok(())
}

/// create 要求全新 root；create_run_in_existing_root 显式复用普通 root。
/// 两个入口都必须独占全新 run，此模块从不打开已有 JSON。
/// manifest 是完成标记；调用者必须调用 finish，并处理其 I/O 错误。
#[must_use = "必须调用 finish 写入 manifest，并处理其 I/O 错误"]
pub struct DeltaRunWriter {
    run: PathBuf,
    window: DeltaWindow,
    results: Vec<Value>,
}

impl DeltaRunWriter {
    pub fn create(root: &Path, window: DeltaWindow) -> Result<Self> {
        window.validate()?;
        validate_root_path(root)?;
        check_directory_ancestors(root.parent().context("root requires parent")?)?;
        fs::create_dir(root).context("delta root must be new and exclusive")?;
        check_directory_ancestors(root)?;
        fs::create_dir(root.join("deltas"))?;
        Self::create_exclusive_run(root, window)
    }

    /// 在已有普通导出 root 下追加全新批次；只允许复用 root 和 deltas 目录。
    /// 同名 run 即使为空或尚无 manifest 也拒绝，不恢复或覆盖旧批次。
    /// 调用方必须先排除源数据库、密钥与缓存路径；本层无法判断目录业务用途。
    pub fn create_run_in_existing_root(root: &Path, window: DeltaWindow) -> Result<Self> {
        window.validate()?;
        validate_root_path(root)?;
        check_directory_ancestors(root)?;
        let deltas = root.join("deltas");
        match fs::create_dir(&deltas) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("cannot create delta directory"),
        }
        Self::create_exclusive_run(root, window)
    }

    fn create_exclusive_run(root: &Path, window: DeltaWindow) -> Result<Self> {
        let deltas = root.join("deltas");
        check_directory_ancestors(&deltas)?;
        let run = deltas.join(&window.run_id);
        // create_dir 是批次独占点；失败时不清理或重用可能属于其它 writer 的目录。
        fs::create_dir(&run).context("delta run must be new and exclusive")?;
        check_directory_ancestors(&run)?;
        fs::create_dir(run.join("chats"))?;
        check_directory_ancestors(&run.join("chats"))?;
        Ok(Self {
            run,
            window,
            results: Vec::new(),
        })
    }

    fn publish(&self, parent: &Path, filename: &str, value: &Value) -> Result<()> {
        safe_component(filename)?;
        check_directory_ancestors(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, value)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        check_directory_ancestors(parent)?;
        temporary
            .persist_noclobber(parent.join(filename))
            .map_err(|e| e.error)?;
        Ok(())
    }

    /// 单个聊天失败不阻止其它聊天；每次调用恰好追加一个 manifest 结果。
    pub fn write_chat(&mut self, chat: &DeltaChat) -> Value {
        let prepared = (|| -> Result<Value> {
            let prepared = prepare_delta(chat, &self.window)?;
            if let Some(document) = prepared.document {
                self.publish(
                    &self.run.join("chats"),
                    &delta_filename(&chat.display_name, chat.is_group, &chat.username),
                    &document,
                )?;
            }
            Ok(prepared.result)
        })();
        let result = prepared
            .unwrap_or_else(|e| failure(&chat.username, &format!("delta export error: {e:#}")));
        self.results.push(result.clone());
        result
    }

    pub fn finish(self) -> Result<PathBuf> {
        let manifest = delta_manifest(&self.window, self.results.len(), &self.results)?;
        self.publish(&self.run, "manifest.json", &manifest)?;
        Ok(self.run.join("manifest.json"))
    }
}

#[cfg(test)]
#[path = "chat_delta_tests.rs"]
mod tests;
