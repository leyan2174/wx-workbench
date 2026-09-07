use super::{now, valid_id, HISTORY_LIMIT, LINE_LIMIT, LOG_LIMIT};
use crate::{
    attachment::local_files::HostOutputGuard, runtime::RuntimeContext, service::protocol::Task,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{Read, Write},
};

pub struct Redactor {
    pattern: regex::Regex,
    secrets: Vec<zeroize::Zeroizing<String>>,
}

impl Redactor {
    pub fn new(runtime: &RuntimeContext) -> Result<Self> {
        fn collect(value: &Value, all: bool, secrets: &mut Vec<zeroize::Zeroizing<String>>) {
            match value {
                Value::String(text) if all && text.len() >= 4 => {
                    secrets.push(zeroize::Zeroizing::new(text.clone()))
                }
                Value::Object(map) => {
                    for (key, value) in map {
                        let key = key.to_lowercase();
                        collect(
                            value,
                            all || ["key", "token", "secret", "password"]
                                .iter()
                                .any(|part| key.contains(part)),
                            secrets,
                        );
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        collect(value, all, secrets);
                    }
                }
                _ => (),
            }
        }
        let mut secrets = Vec::new();
        for (path, all) in [
            (&runtime.config_path, false),
            (&runtime.config.keys_file, true),
        ] {
            if !path.exists() {
                continue;
            }
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            fs::File::open(path)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 4 * 1024 * 1024,
                "Credential file exceeds limit"
            );
            // Invalid keys must not prevent the service from accepting a key-repair task.
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) => collect(&value, all, &mut secrets),
                Err(_) if all => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        if text.len() >= 4 {
                            secrets.push(zeroize::Zeroizing::new(text.into()));
                        }
                    }
                }
                Err(_) => anyhow::bail!("Invalid account configuration"),
            }
        }
        Ok(Self {
            pattern: regex::Regex::new(
                r"(?i)(key|token|secret|password|authorization|credential|bearer|密钥|密码)|[a-f0-9]{24,}|[a-z0-9+/=_-]{32,}",
            )?,
            secrets,
        })
    }

    pub fn clean(&self, text: &str) -> String {
        if self.pattern.is_match(text)
            || self
                .secrets
                .iter()
                .any(|secret| text.contains(secret.as_str()))
        {
            return "[sensitive output redacted]".into();
        }
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        if text.len() <= LINE_LIMIT {
            return text;
        }
        let mut end = LINE_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{} [truncated]", &text[..end])
    }
}

pub fn persist(
    runtime: &RuntimeContext,
    tasks: &VecDeque<Task>,
    requests: &HashMap<String, String>,
) -> Result<()> {
    let guard = HostOutputGuard::new(&runtime.directory)?;
    let path = runtime.directory.join("tasks-history.json");
    guard.verify_replaceable_file(&path)?;
    let bytes = serde_json::to_vec(
        &json!({"version":1,"runtime_id":runtime.id,"tasks":tasks,"requests":requests}),
    )?;
    ensure!(
        bytes.len() <= 20 * 1024 * 1024,
        "Task history exceeds limit"
    );
    let mut file = tempfile::NamedTempFile::new_in(&runtime.directory)?;
    crate::toolkit::private_file::restrict(file.as_file())?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    guard.verify_replaceable_file(&path)?;
    file.persist(&path)?;
    guard.verify()?;
    Ok(())
}

pub fn restore(
    runtime: &RuntimeContext,
    redactor: &Redactor,
) -> Result<(VecDeque<Task>, HashMap<String, String>)> {
    let current = runtime.directory.join("tasks-history.json");
    let path = if current.exists() {
        current
    } else {
        runtime.directory.join("web-history.json")
    };
    if !path.exists() {
        return Ok((VecDeque::new(), HashMap::new()));
    }
    let guard = HostOutputGuard::new(&runtime.directory)?;
    guard.verify_replaceable_file(&path)?;
    let mut bytes = Vec::new();
    fs::File::open(&path)?
        .take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 20 * 1024 * 1024,
        "Task history exceeds limit"
    );
    let value: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        value["runtime_id"] == runtime.id && value["version"] == 1,
        "Task history identity mismatch"
    );
    let mut tasks: VecDeque<Task> = serde_json::from_value(value["tasks"].clone())?;
    ensure!(
        tasks.len() <= HISTORY_LIMIT,
        "Task history count exceeds limit"
    );
    let requests: HashMap<String, String> = match value.get("requests") {
        Some(value) => serde_json::from_value(value.clone())?,
        None => HashMap::new(),
    };
    ensure!(requests.len() <= tasks.len(), "Invalid submission index");
    let mut ids = std::collections::HashSet::new();
    for task in &mut tasks {
        ensure!(
            valid_id(&task.id) && ids.insert(task.id.clone()),
            "Invalid task identity"
        );
        ensure!(
            task.output_dir
                == runtime
                    .root
                    .join("web-output")
                    .join(&runtime.id)
                    .join(&task.id),
            "Task output identity mismatch"
        );
        ensure!(
            task.logs.len() <= LOG_LIMIT && task.options.users.len() <= 200,
            "Task record exceeds limit"
        );
        ensure!(
            matches!(
                task.status.as_str(),
                "queued"
                    | "running"
                    | "cancelling"
                    | "succeeded"
                    | "failed"
                    | "cancelled"
                    | "interrupted"
            ),
            "Unknown task state"
        );
        for log in &mut task.logs {
            log.text = redactor.clean(&log.text);
        }
        if !task.terminal() {
            task.status = "interrupted".into();
            task.finished_at = Some(now());
        }
        if task.error.is_some() {
            task.error = Some("任务未成功完成".into());
        }
    }
    ensure!(
        requests
            .iter()
            .all(|(id, hash)| ids.contains(id) && valid_id(hash)),
        "Invalid submission index"
    );
    guard.verify()?;
    Ok((tasks, requests))
}
