//! Chat-directory publication evidence. No caller paths or independent task queue.
pub(crate) use super::artifact_file::FinalizeControl;
use super::artifact_file::{error, relative, Identity, Reader};
use crate::{
    attachment::local_files::HostOutputGuard,
    runtime::RuntimeContext,
    service::{
        protocol::{valid_task_id, ServiceError, Task},
        task_artifacts::*,
    },
};
use anyhow::{ensure, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const CHECKPOINT_LIMIT: u64 = 4 * 1024 * 1024;
const INVENTORY_LIMIT: u64 = 16 * 1024 * 1024;
const MAX_CHUNKS: usize = 200_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub username: String,
    pub directory: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    version: u32,
    runtime_id: String,
    task_id: String,
    pub candidates: Vec<Candidate>,
    pub result: ExportAllResult,
}

pub(super) fn private_dir(runtime: &RuntimeContext, id: &str) -> Result<PathBuf> {
    ensure!(valid_task_id(id), "Invalid task identity");
    Ok(runtime.directory.join("task-results").join(id))
}

pub(crate) fn prepare(runtime: &RuntimeContext, id: &str) -> Result<()> {
    let guard = HostOutputGuard::new(&runtime.directory)?;
    let parent = runtime.directory.join("task-results");
    match fs::create_dir(&parent) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let parent_guard = HostOutputGuard::new(&parent)?;
    fs::create_dir(private_dir(runtime, id)?)?;
    HostOutputGuard::new(&private_dir(runtime, id)?)?.verify()?;
    parent_guard.verify()?;
    guard.verify()
}

pub(super) fn persist(
    runtime: &RuntimeContext,
    id: &str,
    name: &str,
    value: &impl Serialize,
    limit: u64,
) -> Result<()> {
    let directory = private_dir(runtime, id)?;
    let guard = HostOutputGuard::new(&directory)?;
    let path = directory.join(name);
    guard.verify_replaceable_file(&path)?;
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() as u64 <= limit, "Task result exceeds limit");
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    crate::private_file::restrict(file.as_file())?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    guard.verify_replaceable_file(&path)?;
    file.persist(&path)?;
    guard.verify()
}

pub(super) fn json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    Ok(serde_json::from_slice(
        &Reader::open(path)?.bounded(limit)?,
    )?)
}

impl Checkpoint {
    pub(crate) fn start(
        runtime: &RuntimeContext,
        id: &str,
        dry_run: bool,
        candidates: Vec<Candidate>,
    ) -> Result<Self> {
        ensure!(candidates.len() <= MAX_ARTIFACTS, "Too many chat targets");
        let mut result = ExportAllResult::empty(dry_run);
        result.planned_chats = Some(candidates.len() as u64);
        let checkpoint = Self {
            version: 1,
            runtime_id: runtime.id.clone(),
            task_id: id.into(),
            candidates,
            result,
        };
        checkpoint.save(runtime)?;
        persist(
            runtime,
            id,
            "artifact-index.json",
            &Index::empty(runtime, id),
            MAX_INDEX_BYTES,
        )?;
        Ok(checkpoint)
    }

    pub(crate) fn save(&self, runtime: &RuntimeContext) -> Result<()> {
        ensure!(self.runtime_id == runtime.id, "Checkpoint account mismatch");
        persist(
            runtime,
            &self.task_id,
            "step-report.json",
            self,
            CHECKPOINT_LIMIT,
        )
    }

    pub(crate) fn register_chat(
        &self,
        runtime: &RuntimeContext,
        root: &Path,
        username: &str,
    ) -> Result<()> {
        let candidate = self
            .candidates
            .iter()
            .find(|c| c.username == username)
            .ok_or_else(|| error("result_unavailable"))?;
        let mut index = read_index(runtime, &self.task_id)?;
        let result = append_chat(
            runtime,
            root,
            candidate,
            &mut index,
            &mut FinalizeControl::supervised_worker(),
        );
        // Commit the completed prefix and consumed budget even after a failed hash.
        persist(
            runtime,
            &self.task_id,
            "artifact-index.json",
            &index,
            MAX_INDEX_BYTES,
        )?;
        result
    }

    fn validate(&self, runtime: &RuntimeContext, task: &Task) -> Result<()> {
        ensure!(
            self.version == 1
                && self.runtime_id == runtime.id
                && self.task_id == task.id
                && self.result.dry_run == task.options.dry_run
                && self.result.validate()
                && self.candidates.len() <= MAX_ARTIFACTS
                && self.result.planned_chats == Some(self.candidates.len() as u64),
            "Invalid task checkpoint"
        );
        let mut directories = HashSet::new();
        let mut users = HashSet::new();
        for candidate in &self.candidates {
            let path = relative(&candidate.directory)?;
            ensure!(
                path.components().count() == 1
                    && !candidate.username.is_empty()
                    && directories.insert(candidate.directory.to_lowercase())
                    && users.insert(&candidate.username),
                "Invalid chat candidate"
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Inventory {
    version: u32,
    runtime_id: String,
    username: String,
    files: BTreeMap<String, String>,
    messages: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) artifact: Artifact,
    pub(super) relative: String,
    pub(super) identity: Identity,
    pub(super) chunks: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    runtime_id: String,
    task_id: String,
    complete: bool,
    entries: Vec<Entry>,
    #[serde(default)]
    chats: Vec<PublishedChat>,
    #[serde(default = "hash_budget")]
    chunks_left: usize,
}

fn hash_budget() -> usize {
    MAX_CHUNKS
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedChat {
    directory: String,
    username: String,
    messages: u64,
    media_issues: u64,
}

impl Index {
    fn empty(runtime: &RuntimeContext, task_id: &str) -> Self {
        Self {
            version: 1,
            runtime_id: runtime.id.clone(),
            task_id: task_id.into(),
            complete: false,
            entries: Vec::new(),
            chats: Vec::new(),
            chunks_left: MAX_CHUNKS,
        }
    }
}

fn append_chat(
    runtime: &RuntimeContext,
    root: &Path,
    candidate: &Candidate,
    index: &mut Index,
    control: &mut FinalizeControl,
) -> Result<()> {
    if index
        .chats
        .iter()
        .any(|c| c.directory == candidate.directory && c.username == candidate.username)
    {
        return Ok(());
    }
    let (mut entries, messages, media_issues) = collect_chat(
        runtime,
        root,
        candidate,
        MAX_ARTIFACTS - index.entries.len(),
        &mut index.chunks_left,
        control,
    )?;
    let mut ids: HashSet<String> = index
        .entries
        .iter()
        .map(|e| e.artifact.artifact_id.clone())
        .collect();
    for entry in &mut entries {
        while !ids.insert(entry.artifact.artifact_id.clone()) {
            control.check()?;
            entry.artifact.artifact_id = crate::service::transport::random_secret()?.to_string();
        }
    }
    let old_len = index.entries.len();
    index.entries.extend(entries);
    index.chats.push(PublishedChat {
        directory: candidate.directory.clone(),
        username: candidate.username.clone(),
        messages,
        media_issues,
    });
    if serde_json::to_vec(index)?.len() as u64 > MAX_INDEX_BYTES {
        index.entries.truncate(old_len);
        index.chats.pop();
        return Err(error("artifact_limit_exceeded").into());
    }
    Ok(())
}

fn metadata(name: &str) -> (&'static str, &'static str) {
    match name {
        "_directory_export.json" => ("export_inventory", "application/json"),
        "_media_manifest.json" => ("media_manifest", "application/json"),
        "_voice_manifest.json" => ("voice_manifest", "application/json"),
        ".info" => ("chat_info", "text/plain"),
        _ if !name.contains('/') && name.ends_with(".json") => {
            ("chat_document", "application/json")
        }
        _ if !name.contains('/') && name.ends_with(".csv") => ("chat_document", "text/csv"),
        _ if !name.contains('/') && name.ends_with(".html") => ("chat_document", "text/html"),
        _ => ("media", "application/octet-stream"),
    }
}

pub(super) fn register(
    path: &Path,
    rel: String,
    name: &str,
    expected: Option<&str>,
    chunks_left: &mut usize,
    control: &mut FinalizeControl,
) -> Result<Entry> {
    control.check()?;
    let mut reader = Reader::open(path)?;
    let (sha256, chunks) = reader.hashes(chunks_left, control)?;
    if expected.is_some_and(|hash| hash != sha256) {
        return Err(error("artifact_changed").into());
    }
    let (role, media_type) = metadata(name);
    let artifact = Artifact {
        artifact_id: crate::service::transport::random_secret()?.to_string(),
        name: path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| error("artifact_unsafe"))?
            .into(),
        role: role.into(),
        media_type: media_type.into(),
        size: reader.identity.size,
        sha256,
    };
    Ok(Entry {
        artifact,
        relative: rel,
        identity: reader.identity,
        chunks,
    })
}

fn collect_chat(
    runtime: &RuntimeContext,
    root: &Path,
    candidate: &Candidate,
    left: usize,
    chunks_left: &mut usize,
    control: &mut FinalizeControl,
) -> Result<(Vec<Entry>, u64, u64)> {
    control.check()?;
    let directory = root.join(relative(&candidate.directory)?);
    let guard = HostOutputGuard::new(&directory)?;
    let mut binding_file = Reader::open(&directory.join("_source_binding.json"))?;
    let binding: crate::infrastructure::output_tree::Binding = serde_json::from_slice(
        &binding_file.bounded_controlled(64 * 1024, chunks_left, control)?,
    )?;
    ensure!(
        binding.version == 1
            && binding.tree_kind == "chat_directory"
            && binding.source_kind == "runtime"
            && binding.source_id == runtime.id
            && binding.user_name == candidate.username,
        "Chat binding mismatch"
    );
    // Keep the published manifest pinned throughout registration, not just during parsing.
    let mut manifest = Reader::open(&directory.join("_directory_export.json"))?;
    let inventory_bytes = manifest.bounded_controlled(INVENTORY_LIMIT, chunks_left, control)?;
    let inventory: Inventory = serde_json::from_slice(&inventory_bytes)?;
    ensure!(
        inventory.version == 1
            && inventory.runtime_id == runtime.id
            && inventory.username == candidate.username,
        "Inventory account mismatch"
    );
    if inventory.files.len().saturating_add(1) > left {
        return Err(error("artifact_limit_exceeded").into());
    }
    let mut entries = Vec::new();
    let mut aliases = HashSet::new();
    for (name, hash) in &inventory.files {
        control.check()?;
        let member = relative(name)?;
        ensure!(
            valid_task_id(hash) && aliases.insert(name.to_lowercase()),
            "Invalid inventory member"
        );
        ensure!(
            !matches!(
                name.as_str(),
                "_directory_export.json" | "_source_binding.json" | ".wx-sns-publish.lock"
            ),
            "Private file in inventory"
        );
        crate::infrastructure::publication::validate_export_target(
            runtime,
            &directory.join(&member),
        )?;
        entries.push(register(
            &directory.join(member),
            format!("{}/{}", candidate.directory, name),
            name,
            Some(hash),
            chunks_left,
            control,
        )?);
    }
    let media_name = "_media_manifest.json";
    let media_entry = entries
        .iter()
        .find(|entry| entry.relative == format!("{}/{}", candidate.directory, media_name))
        .ok_or_else(|| error("result_unavailable"))?;
    let mut media = Reader::open(&directory.join(media_name))?;
    let bytes = media.bounded_controlled(INVENTORY_LIMIT, chunks_left, control)?;
    ensure!(
        digest_controlled(&bytes, control)? == media_entry.artifact.sha256,
        "Media manifest changed"
    );
    let media: serde_json::Value = serde_json::from_slice(&bytes)?;
    let issues = media["issues"]
        .as_u64()
        .ok_or_else(|| error("result_unavailable"))?;
    entries.push(register(
        &directory.join("_directory_export.json"),
        format!("{}/_directory_export.json", candidate.directory),
        "_directory_export.json",
        Some(&digest_controlled(&inventory_bytes, control)?),
        chunks_left,
        control,
    )?);
    manifest.verify()?;
    binding_file.verify()?;
    guard.verify()?;
    control.check()?;
    Ok((entries, inventory.messages.len() as u64, issues))
}

fn digest_controlled(bytes: &[u8], control: &FinalizeControl) -> Result<String> {
    let mut hash = Sha256::new();
    for block in bytes.chunks(CHUNK_BYTES as usize) {
        control.check()?;
        hash.update(block);
    }
    control.check()?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Reuse the worker's durable prefix; recover only published checkpoint gaps.
pub(super) fn finalize_task(
    runtime: &RuntimeContext,
    task: &Task,
    control: &mut FinalizeControl,
) -> Result<TaskResult> {
    match task.kind {
        crate::service::protocol::Kind::ExportAll => {
            finalize(runtime, task, control).map(TaskResult::ChatDirectory)
        }
        crate::service::protocol::Kind::ExportHistory => {
            super::history_artifacts::finalize(runtime, task, control).map(TaskResult::ChatHistory)
        }
        _ => Err(error("result_unavailable").into()),
    }
}

pub(crate) fn finalize(
    runtime: &RuntimeContext,
    task: &Task,
    control: &mut FinalizeControl,
) -> Result<ExportAllResult> {
    let checkpoint: Checkpoint = json(
        &private_dir(runtime, &task.id)?.join("step-report.json"),
        CHECKPOINT_LIMIT,
    )?;
    checkpoint.validate(runtime, task)?;
    let mut index = read_index(runtime, &task.id)?;
    let candidates: HashSet<_> = checkpoint
        .candidates
        .iter()
        .map(|c| (&c.directory, &c.username))
        .collect();
    ensure!(
        index
            .chats
            .iter()
            .all(|chat| candidates.contains(&(&chat.directory, &chat.username))),
        "Index candidate mismatch"
    );
    let mut result = checkpoint.result;
    result.artifacts_complete = true;
    result.diagnostics.clear();
    let root = task.output_dir.join("chats");
    let mut stopped = false;
    // Payload hashes were committed by the supervised worker. Recheck identity only;
    // every download still verifies its requested blocks against the committed hashes.
    for entry in &index.entries {
        if let Err(e) = control.check() {
            diagnostic(&mut result, &e.code);
            result.artifacts_complete = false;
            stopped = true;
            break;
        }
        let checked = (|| -> Result<()> {
            let reader = Reader::open(&root.join(relative(&entry.relative)?))?;
            ensure!(
                reader.identity == entry.identity,
                "Registered identity changed"
            );
            reader.verify()?;
            Ok(())
        })();
        if checked.is_err() {
            result.artifacts_complete = false;
            diagnostic(&mut result, "artifact_changed");
        }
    }
    if !result.dry_run && !stopped {
        for candidate in &checkpoint.candidates {
            if let Err(e) = control.check() {
                diagnostic(&mut result, &e.code);
                result.artifacts_complete = false;
                break;
            }
            if index
                .chats
                .iter()
                .any(|c| c.directory == candidate.directory)
            {
                continue;
            }
            let manifest = root
                .join(relative(&candidate.directory)?)
                .join("_directory_export.json");
            if matches!(fs::symlink_metadata(&manifest), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            if let Err(e) = append_chat(runtime, &root, candidate, &mut index, control) {
                result.artifacts_complete = false;
                let code = e
                    .downcast_ref::<ServiceError>()
                    .map_or("result_unavailable", |e| e.code.as_str());
                diagnostic(&mut result, code);
                if control.check().is_err() {
                    break;
                }
            }
        }
    }
    if let Err(e) = control.check() {
        result.artifacts_complete = false;
        let code = stop_diagnostic(&e.code);
        if !result.diagnostics.iter().any(|d| d.code == code) {
            diagnostic(&mut result, &e.code);
        }
    }
    result.exported_chats = index.chats.len() as u64;
    result.messages = index.chats.iter().map(|c| c.messages).sum();
    result.media_issues = index.chats.iter().map(|c| c.media_issues).sum();
    result.artifact_count = index.entries.len() as u64;
    if result.failed_chats > 0 {
        result.diagnostics.push(Diagnostic {
            code: "chat_export_failed".into(),
            count: result.failed_chats,
        });
    }
    if result.media_issues > 0 {
        result.diagnostics.push(Diagnostic {
            code: "media_unavailable".into(),
            count: result.media_issues,
        });
    }
    if !result.finalized
        && !result
            .diagnostics
            .iter()
            .any(|d| d.code == "export_interrupted")
    {
        diagnostic(&mut result, "export_interrupted");
    }
    if result.finalized
        && !result.dry_run
        && result.exported_chats.saturating_add(result.failed_chats)
            != result.planned_chats.unwrap_or(0)
    {
        result.artifacts_complete = false;
        diagnostic(&mut result, "result_unavailable");
    }
    result.outcome = if result.finalized
        && result.failed_chats == 0
        && result.media_issues == 0
        && result.artifacts_complete
        && (result.dry_run || result.planned_chats == Some(result.exported_chats))
    {
        Some(ExportOutcome::Success)
    } else if result.exported_chats > 0 {
        Some(ExportOutcome::Partial)
    } else if result.finalized || result.failed_chats > 0 {
        Some(ExportOutcome::Failure)
    } else {
        None
    };
    ensure!(result.validate(), "Invalid result summary");
    index.complete = result.artifacts_complete;
    // This bounded metadata commit is awaited even on cancellation; no detached work.
    persist(
        runtime,
        &task.id,
        "artifact-index.json",
        &index,
        MAX_INDEX_BYTES,
    )?;
    Ok(result)
}

fn stop_diagnostic(code: &str) -> &str {
    match code {
        "artifact_finalization_cancelled" => "export_interrupted",
        "artifact_finalization_timeout" => "artifact_finalization_timeout",
        _ => code,
    }
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;

fn diagnostic(result: &mut ExportAllResult, code: &str) {
    let code = stop_diagnostic(code);
    let code = match code {
        "artifact_limit_exceeded"
        | "artifact_changed"
        | "artifact_unsafe"
        | "artifact_unavailable"
        | "artifact_finalization_timeout"
        | "export_interrupted" => code,
        _ => "result_unavailable",
    };
    if let Some(item) = result.diagnostics.iter_mut().find(|d| d.code == code) {
        item.count += 1;
    } else {
        result.diagnostics.push(Diagnostic {
            code: code.into(),
            count: 1,
        });
    }
}

fn read_index(runtime: &RuntimeContext, id: &str) -> Result<Index, ServiceError> {
    let path = private_dir(runtime, id).map_err(|_| error("result_unavailable"))?;
    let index: Index = json(&path.join("artifact-index.json"), MAX_INDEX_BYTES)
        .map_err(|_| error("result_unavailable"))?;
    if index.version != 1
        || index.runtime_id != runtime.id
        || index.task_id != id
        || index.entries.len() > MAX_ARTIFACTS
        || index.chunks_left > MAX_CHUNKS
        || index.chats.len() > MAX_ARTIFACTS
    {
        return Err(error("result_unavailable"));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for entry in &index.entries {
        if !valid_task_id(&entry.artifact.artifact_id)
            || !ids.insert(&entry.artifact.artifact_id)
            || !names.insert(entry.relative.to_lowercase())
            || relative(&entry.relative).is_err()
            || !valid_task_id(&entry.artifact.sha256)
            || entry.artifact.size != entry.identity.size
            || entry.artifact.size > MAX_SAFE_INTEGER
            || entry.chunks.len() as u64 != entry.artifact.size.div_ceil(CHUNK_BYTES as u64)
            || entry.chunks.iter().any(|hash| !valid_task_id(hash))
        {
            return Err(error("result_unavailable"));
        }
    }
    let mut chats = HashSet::new();
    for chat in &index.chats {
        if !chats.insert(&chat.directory)
            || relative(&chat.directory).is_err()
            || chat.messages > MAX_SAFE_INTEGER / MAX_ARTIFACTS as u64
            || chat.media_issues > MAX_SAFE_INTEGER / MAX_ARTIFACTS as u64
        {
            return Err(error("result_unavailable"));
        }
    }
    Ok(index)
}

fn load(runtime: &RuntimeContext, task: &Task) -> Result<Index, ServiceError> {
    let index = read_index(runtime, &task.id)?;
    if task.result.as_ref().is_none_or(|result| {
        !result.validate(task.kind)
            || !matches!(result, TaskResult::ChatDirectory(_))
            || result.artifact_count() != index.entries.len() as u64
            || result.artifacts_complete() != index.complete
    }) {
        return Err(error("result_unavailable"));
    }
    Ok(index)
}

pub(super) fn list(
    runtime: &RuntimeContext,
    task: &Task,
    offset: u64,
    limit: u32,
) -> Result<ArtifactsPage, ServiceError> {
    if task.kind == crate::service::protocol::Kind::ExportHistory {
        return super::history_artifacts::list(runtime, task, offset, limit);
    }
    let index = load(runtime, task)?;
    if !(1..=MAX_LIST_ITEMS).contains(&limit) || offset > index.entries.len() as u64 {
        return Err(error("invalid_artifact_request"));
    }
    let total = index.entries.len() as u64;
    let items: Vec<_> = index
        .entries
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|entry| entry.artifact)
        .collect();
    let next = offset + items.len() as u64;
    Ok(ArtifactsPage {
        version: 1,
        task_id: task.id.clone(),
        scope: "chat_directory".into(),
        items,
        offset,
        next_offset: (next < total).then_some(next),
        total,
        complete: index.complete,
    })
}

pub(super) fn read(
    runtime: &RuntimeContext,
    task: &Task,
    id: &str,
    offset: u64,
    max_bytes: u32,
) -> Result<ArtifactBytes, ServiceError> {
    if task.kind == crate::service::protocol::Kind::ExportHistory {
        return super::history_artifacts::read(runtime, task, id, offset, max_bytes);
    }
    if !valid_task_id(id) || !(1..=CHUNK_BYTES).contains(&max_bytes) {
        return Err(error("invalid_artifact_request"));
    }
    let index = load(runtime, task)?;
    let entry = index
        .entries
        .iter()
        .find(|entry| entry.artifact.artifact_id == id)
        .ok_or_else(|| {
            ServiceError::new("not_found", "Artifact not found for this account task")
        })?;
    read_entry(
        runtime,
        &task.id,
        &task.output_dir.join("chats"),
        entry,
        offset,
        max_bytes,
    )
}

pub(super) fn read_entry(
    runtime: &RuntimeContext,
    task_id: &str,
    root: &Path,
    entry: &Entry,
    offset: u64,
    max_bytes: u32,
) -> Result<ArtifactBytes, ServiceError> {
    if !(1..=CHUNK_BYTES).contains(&max_bytes) {
        return Err(error("invalid_artifact_request"));
    }
    let size = entry.artifact.size;
    if offset > size {
        return Err(error("invalid_artifact_request"));
    }
    let path = root.join(relative(&entry.relative)?);
    crate::infrastructure::publication::validate_export_target(runtime, &path)
        .map_err(|_| error("artifact_unsafe"))?;
    let mut reader = Reader::open(&path)?;
    if reader.identity != entry.identity {
        return Err(error("artifact_changed"));
    }
    let end = offset + (size - offset).min(max_bytes as u64);
    let mut bytes = Vec::with_capacity((end - offset) as usize);
    let mut block = offset / CHUNK_BYTES as u64;
    while block * (CHUNK_BYTES as u64) < end {
        let start = block * CHUNK_BYTES as u64;
        let data = reader.range(start, (size - start).min(CHUNK_BYTES as u64) as usize)?;
        if entry
            .chunks
            .get(block as usize)
            .is_none_or(|hash| *hash != format!("{:x}", Sha256::digest(&data)))
        {
            return Err(error("artifact_changed"));
        }
        let begin = offset.saturating_sub(start) as usize;
        let finish = (end - start).min(data.len() as u64) as usize;
        bytes.extend_from_slice(&data[begin..finish]);
        block += 1;
    }
    reader.verify()?;
    Ok(ArtifactBytes {
        version: 1,
        task_id: task_id.into(),
        artifact_id: entry.artifact.artifact_id.clone(),
        offset,
        bytes_read: bytes.len() as u64,
        next_offset: end,
        size,
        sha256: entry.artifact.sha256.clone(),
        encoding: "base64".into(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        eof: end == size,
    })
}
