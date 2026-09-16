use super::{now, valid_id, HISTORY_LIMIT, LINE_LIMIT, LOG_LIMIT};
use crate::{
    attachment::local_files::HostOutputGuard, runtime::RuntimeContext, service::protocol::Task,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
    pub fn new(
        runtime: &RuntimeContext,
        snapshot: Option<&crate::key_store::Snapshot>,
    ) -> Result<Self> {
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
        if runtime.config_path.exists() {
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            fs::File::open(&runtime.config_path)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 4 * 1024 * 1024,
                "Credential file exceeds limit"
            );
            let value = serde_json::from_slice::<Value>(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid account configuration"))?;
            collect(&value, false, &mut secrets);
        }
        // The execution host supplies its account snapshot; redaction never opens a key store.
        if let Some(snapshot) = snapshot {
            secrets.extend(
                snapshot
                    .database_keys()
                    .into_values()
                    .map(zeroize::Zeroizing::new),
            );
            if let Some((mut aes, _)) = snapshot.image_key() {
                if let Ok(text) = std::str::from_utf8(&aes) {
                    secrets.push(zeroize::Zeroizing::new(text.to_owned()));
                }
                use zeroize::Zeroize;
                aes.zeroize();
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
    crate::private_file::restrict(file.as_file())?;
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
    let rows = value["tasks"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid task history"))?;
    ensure!(
        rows.len() <= HISTORY_LIMIT,
        "Task history count exceeds limit"
    );
    let mut requests: HashMap<String, String> = match value.get("requests") {
        Some(value) => serde_json::from_value(value.clone())?,
        None => HashMap::new(),
    };
    ensure!(requests.len() <= rows.len(), "Invalid submission index");
    ensure!(
        requests
            .iter()
            .all(|(id, hash)| valid_id(id) && valid_id(hash)),
        "Invalid submission index"
    );
    let mut tasks = VecDeque::new();
    let mut retired = std::collections::HashSet::new();
    let mut migrated = false;
    for row in rows {
        let mut row = row.clone();
        if matches!(
            row["kind"].as_str(),
            Some(
                "wxwork_decrypt"
                    | "wxwork_export"
                    | "wxwork_discover"
                    | "wxwork_scan"
                    | "wxwork_run"
            )
        ) {
            let id = row["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid retired task identity"))?;
            ensure!(
                valid_id(id) && retired.insert(id.to_owned()),
                "Invalid retired task identity"
            );
            migrated = true;
            continue;
        }
        if let Some(options) = row["options"].as_object_mut() {
            if let Some(value) = options.remove("all_conversations") {
                ensure!(value == false, "Invalid historical personal task options");
                migrated = true;
            }
        }
        tasks.push_back(serde_json::from_value::<Task>(row)?);
    }
    requests.retain(|id, _| !retired.contains(id));
    let mut ids = std::collections::HashSet::new();
    for task in &mut tasks {
        ensure!(
            valid_id(&task.id) && !retired.contains(&task.id) && ids.insert(task.id.clone()),
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
    if migrated {
        // Preserve the exact pre-migration journal before startup rewrites active history.
        let backup = runtime.directory.join(format!(
            "tasks-history-retired-{:x}.json",
            Sha256::digest(&bytes)
        ));
        guard.verify_replaceable_file(&backup)?;
        if backup.exists() {
            ensure!(
                fs::metadata(&backup)?.len() == bytes.len() as u64,
                "History archive differs"
            );
            let mut existing = Vec::new();
            fs::File::open(&backup)?
                .take(20 * 1024 * 1024 + 1)
                .read_to_end(&mut existing)?;
            ensure!(existing == bytes, "History archive differs");
        } else {
            let mut file = tempfile::NamedTempFile::new_in(&runtime.directory)?;
            crate::private_file::restrict(file.as_file())?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            guard.verify_replaceable_file(&backup)?;
            file.persist_noclobber(&backup)?;
        }
    }
    guard.verify()?;
    Ok((tasks, requests))
}
