//! Durable raw-voice groups. A published but uncommitted file is never an artifact.
use super::{
    artifact_file::{error, relative, Reader},
    artifacts::{self, Entry, FinalizeControl},
};
use crate::{
    runtime::RuntimeContext,
    service::{
        config_pin::ConfigPin,
        protocol::{valid_task_id, Kind, Options, ServiceError, Submission, Task},
        task_artifacts::*,
        voice_export::{Request, VoiceExportResult},
    },
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const HASH_BLOCKS: usize = 200_000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    entries: [Entry; 2],
    associated: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    version: u32,
    runtime_id: String,
    task_id: String,
    request_sha256: String,
    config_sha256: String,
    result: VoiceExportResult,
    groups: Vec<Group>,
    summary: Option<Entry>,
    chunks_left: usize,
}
pub(crate) fn root(runtime: &RuntimeContext, id: &str) -> Result<PathBuf> {
    ensure!(valid_task_id(id), "Invalid voice task identity");
    Ok(runtime
        .root
        .join("web-output")
        .join(&runtime.id)
        .join(id)
        .join("voices"))
}
fn signature(request: &Request) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&Submission {
            kind: Kind::ExportVoices,
            options: Options {
                voice_export: Some(request.clone()),
                ..Default::default()
            }
        })?)
    ))
}
fn save(runtime: &RuntimeContext, index: &Publication) -> Result<()> {
    artifacts::persist(
        runtime,
        &index.task_id,
        "artifact-index.json",
        index,
        MAX_INDEX_BYTES,
    )
}
pub(super) fn start(runtime: &RuntimeContext, task: &Task) -> Result<()> {
    let request = selection(task)?;
    ensure!(
        task.output_dir.join("voices") == root(runtime, &task.id)?,
        "Voice output mismatch"
    );
    save(
        runtime,
        &Publication {
            version: 1,
            runtime_id: runtime.id.clone(),
            task_id: task.id.clone(),
            request_sha256: signature(request)?,
            config_sha256: ConfigPin::new(runtime)?.fingerprint()?,
            result: VoiceExportResult::empty(request.clone())?,
            groups: Vec::new(),
            summary: None,
            chunks_left: HASH_BLOCKS,
        },
    )
}
fn selection(task: &Task) -> Result<&Request> {
    ensure!(task.kind == Kind::ExportVoices, "Wrong voice task kind");
    crate::service::plan::validate(
        &Submission {
            kind: task.kind,
            options: task.options.clone(),
        },
        &Default::default(),
    )?;
    task.options
        .voice_export
        .as_ref()
        .ok_or_else(|| error("result_unavailable").into())
}
fn entries(index: &Publication) -> impl Iterator<Item = &Entry> {
    index
        .groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .chain(index.summary.iter())
}
fn load_bound(runtime: &RuntimeContext, id: &str, request: &Request) -> Result<Publication> {
    let index: Publication = artifacts::json(
        &artifacts::private_dir(runtime, id)?.join("artifact-index.json"),
        MAX_INDEX_BYTES,
    )?;
    ensure!(
        index.version == 1
            && index.runtime_id == runtime.id
            && index.task_id == id
            && index.request_sha256 == signature(request)?
            && index.result.selection.request == *request
            && index.config_sha256 == ConfigPin::new(runtime)?.fingerprint()?
            && index.result.validate()
            && index.chunks_left <= HASH_BLOCKS
            && index.result.exported == index.groups.len() as u64
            && index.result.associated
                == index.groups.iter().filter(|group| group.associated).count() as u64
            && index.result.artifact_count
                == (index.groups.len() * 2 + usize::from(index.summary.is_some())) as u64,
        "Invalid voice publication binding"
    );
    let mut ids = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for entry in entries(&index) {
        let rel = relative(&entry.relative)?;
        ensure!(
            ids.insert(entry.artifact.artifact_id.clone())
                && paths.insert(entry.relative.clone())
                && rel.file_name().and_then(|s| s.to_str()) == Some(entry.artifact.name.as_str())
                && valid_task_id(&entry.artifact.artifact_id)
                && valid_task_id(&entry.artifact.sha256)
                && entry.identity.size == entry.artifact.size
                && entry.artifact.size <= MAX_SAFE_INTEGER
                && entry.chunks.len() as u64 == entry.artifact.size.div_ceil(CHUNK_BYTES as u64)
                && entry.chunks.iter().all(|hash| valid_task_id(hash)),
            "Invalid voice artifact"
        );
    }
    for group in &index.groups {
        let [audio, evidence] = &group.entries;
        ensure!(
            audio.relative.strip_suffix(".silk") == evidence.relative.strip_suffix(".voice.json")
                && audio.relative.ends_with(".silk")
                && evidence.relative.ends_with(".voice.json")
                && audio.artifact.role == "media"
                && audio.artifact.media_type == "application/octet-stream"
                && evidence.artifact.role == "voice_manifest"
                && evidence.artifact.media_type == "application/json",
            "Invalid voice file group"
        );
    }
    if let Some(summary) = &index.summary {
        ensure!(
            summary.relative == "_voice_export_summary.json"
                && summary.artifact.role == "voice_manifest"
                && summary.artifact.media_type == "application/json",
            "Invalid voice summary"
        );
    }
    Ok(index)
}
pub(crate) fn selected(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
    count: usize,
    username: Option<String>,
) -> Result<()> {
    let mut index = load_bound(runtime, id, request)?;
    ensure!(
        index.result.selected_rows.is_none() && index.groups.is_empty(),
        "Voice selection already recorded"
    );
    index.result.selected_rows = Some(count.try_into()?);
    index.result.selection.target_username = username;
    ensure!(index.result.validate(), "Invalid voice selection count");
    save(runtime, &index)
}
fn register(
    runtime: &RuntimeContext,
    index: &mut Publication,
    path: &Path,
    expected: &str,
    manifest: bool,
) -> Result<Entry> {
    let rel = path
        .strip_prefix(root(runtime, &index.task_id)?)?
        .to_str()
        .context("Invalid voice filename")?
        .replace('\\', "/");
    relative(&rel)?;
    ensure!(
        !entries(index).any(|entry| entry.relative == rel),
        "Voice artifact already registered"
    );
    let mut entry = artifacts::register(
        path,
        rel,
        path.file_name()
            .and_then(|s| s.to_str())
            .context("Invalid voice filename")?,
        Some(expected),
        &mut index.chunks_left,
        &mut FinalizeControl::supervised_worker(),
    )?;
    if manifest {
        entry.artifact.role = "voice_manifest".into();
        entry.artifact.media_type = "application/json".into();
    } else {
        entry.artifact.role = "media".into();
        entry.artifact.media_type = "application/octet-stream".into();
    }
    while entries(index).any(|known| known.artifact.artifact_id == entry.artifact.artifact_id) {
        entry.artifact.artifact_id = crate::service::transport::random_secret()?.to_string();
    }
    Ok(entry)
}
pub(crate) fn published_group(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
    files: [(&Path, &str); 2],
    associated: bool,
) -> Result<()> {
    let pin = ConfigPin::new(runtime)?;
    let mut index = load_bound(runtime, id, request)?;
    ensure!(
        !index.result.finalized && index.summary.is_none(),
        "Voice publication already finished"
    );
    if index.result.artifact_count + 3 > MAX_ARTIFACTS as u64 {
        return Err(error("artifact_limit_exceeded").into());
    }
    let registered = (|| -> Result<[Entry; 2]> {
        let audio = register(runtime, &mut index, files[0].0, files[0].1, false)?;
        let mut evidence = register(runtime, &mut index, files[1].0, files[1].1, true)?;
        while evidence.artifact.artifact_id == audio.artifact.artifact_id {
            evidence.artifact.artifact_id = crate::service::transport::random_secret()?.to_string();
        }
        Ok([audio, evidence])
    })();
    let pair = match registered {
        Ok(pair) => pair,
        Err(error) => {
            save(runtime, &index)?;
            return Err(error);
        }
    };
    let before = index.result.clone();
    index.groups.push(Group {
        entries: pair,
        associated,
    });
    index.result.exported += 1;
    index.result.associated += u64::from(associated);
    index.result.unproven += u64::from(!associated);
    index.result.incomplete_items += u64::from(!associated);
    index.result.artifact_count += 2;
    if !index.result.validate() || serde_json::to_vec(&index)?.len() as u64 > MAX_INDEX_BYTES {
        index.groups.pop();
        index.result = before;
        save(runtime, &index)?;
        return Err(error("artifact_limit_exceeded").into());
    }
    pin.verify(runtime)?;
    save(runtime, &index)?;
    pin.verify(runtime)
}
pub(crate) fn finish(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
    incomplete: u64,
    budget_failed: bool,
    summary_sha256: &str,
) -> Result<VoiceExportResult> {
    let pin = ConfigPin::new(runtime)?;
    let mut index = load_bound(runtime, id, request)?;
    ensure!(
        index.summary.is_none() && !index.result.finalized,
        "Voice summary already registered"
    );
    let path = root(runtime, id)?.join("_voice_export_summary.json");
    let registered = register(runtime, &mut index, &path, summary_sha256, true);
    let entry = match registered {
        Ok(entry) => entry,
        Err(error) => {
            save(runtime, &index)?;
            return Err(error);
        }
    };
    let before = index.result.clone();
    index.summary = Some(entry);
    index.result.incomplete_items = incomplete;
    index.result.finalized = true;
    index.result.artifact_count += 1;
    index.result.artifacts_complete = !budget_failed;
    index.result.outcome = Some(if incomplete == 0 && !budget_failed {
        ExportOutcome::Success
    } else if index.result.exported > 0 {
        ExportOutcome::Partial
    } else {
        ExportOutcome::Failure
    });
    if budget_failed {
        diagnostic(&mut index.result, "voice_budget_exceeded");
    }
    if index.result.unproven > 0 {
        diagnostic(&mut index.result, "voice_unproven");
    }
    if !index.result.validate() || serde_json::to_vec(&index)?.len() as u64 > MAX_INDEX_BYTES {
        index.summary = None;
        index.result = before;
        save(runtime, &index)?;
        return Err(error("artifact_limit_exceeded").into());
    }
    pin.verify(runtime)?;
    save(runtime, &index)?;
    pin.verify(runtime)?;
    Ok(index.result)
}
fn diagnostic(result: &mut VoiceExportResult, code: &str) {
    if !result.diagnostics.iter().any(|item| item.code == code) {
        result.diagnostics.push(Diagnostic {
            code: code.into(),
            count: 1,
        });
    }
}
pub(crate) fn failed(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
    budget: bool,
) -> Result<()> {
    let mut index = load_bound(runtime, id, request)?;
    index.result.artifacts_complete = false;
    index.result.incomplete_items = index.result.incomplete_items.max(
        index
            .result
            .selected_rows
            .unwrap_or(0)
            .saturating_sub(index.result.exported)
            + index.result.unproven,
    );
    diagnostic(
        &mut index.result,
        if budget {
            "voice_budget_exceeded"
        } else {
            "voice_export_failed"
        },
    );
    save(runtime, &index)
}
pub(super) fn finalize(
    runtime: &RuntimeContext,
    task: &Task,
    control: &mut FinalizeControl,
) -> Result<VoiceExportResult> {
    let mut index = load_bound(runtime, &task.id, selection(task)?)?;
    let verified = (|| -> Result<()> {
        for entry in entries(&index) {
            control.check()?;
            let reader = Reader::open(&root(runtime, &task.id)?.join(relative(&entry.relative)?))?;
            ensure!(reader.identity == entry.identity, "Voice artifact changed");
            reader.verify()?;
        }
        control.check()?;
        Ok(())
    })();
    if let Err(failure) = verified {
        let code = match failure
            .downcast_ref::<ServiceError>()
            .map(|e| e.code.as_str())
        {
            Some("artifact_finalization_cancelled") => "export_interrupted",
            Some("artifact_finalization_timeout") => "artifact_finalization_timeout",
            Some("artifact_unsafe") => "artifact_unsafe",
            Some("artifact_unavailable") => "artifact_unavailable",
            _ => "artifact_changed",
        };
        index.result.artifacts_complete = false;
        diagnostic(&mut index.result, code);
    }
    if !index.result.finalized {
        index.result.incomplete_items = index.result.incomplete_items.max(
            index
                .result
                .selected_rows
                .unwrap_or(0)
                .saturating_sub(index.result.exported)
                + index.result.unproven,
        );
        diagnostic(&mut index.result, "voice_export_failed");
    }
    if !index.result.artifacts_complete {
        index.result.outcome = Some(if index.groups.is_empty() {
            ExportOutcome::Failure
        } else {
            ExportOutcome::Partial
        });
    }
    ensure!(index.result.validate(), "Invalid voice final result");
    save(runtime, &index)?;
    Ok(index.result)
}
fn load(runtime: &RuntimeContext, task: &Task) -> Result<Publication, ServiceError> {
    let index = load_bound(
        runtime,
        &task.id,
        selection(task).map_err(|_| error("result_unavailable"))?,
    )
    .map_err(|_| error("result_unavailable"))?;
    if task.output_dir.join("voices")
        != root(runtime, &task.id).map_err(|_| error("result_unavailable"))?
    {
        return Err(error("result_unavailable"));
    }
    match &task.result {
        Some(TaskResult::RawVoices(result))
            if result.validate()
                && serde_json::to_value(result).ok()
                    == serde_json::to_value(&index.result).ok() =>
        {
            Ok(index)
        }
        _ => Err(error("result_unavailable")),
    }
}
pub(super) fn list(
    runtime: &RuntimeContext,
    task: &Task,
    offset: u64,
    limit: u32,
) -> Result<ArtifactsPage, ServiceError> {
    let index = load(runtime, task)?;
    let total = index.result.artifact_count;
    if offset > total || !(1..=MAX_LIST_ITEMS).contains(&limit) {
        return Err(error("invalid_artifact_request"));
    }
    let offset_usize = usize::try_from(offset).map_err(|_| error("invalid_artifact_request"))?;
    let items = entries(&index)
        .skip(offset_usize)
        .take(limit as usize)
        .map(|entry| entry.artifact.clone())
        .collect::<Vec<_>>();
    let next = offset + items.len() as u64;
    Ok(ArtifactsPage {
        version: 1,
        task_id: task.id.clone(),
        scope: "raw_voices".into(),
        items,
        offset,
        next_offset: (next < total).then_some(next),
        total,
        complete: index.result.artifacts_complete,
    })
}
pub(super) fn read(
    runtime: &RuntimeContext,
    task: &Task,
    id: &str,
    offset: u64,
    max_bytes: u32,
) -> Result<ArtifactBytes, ServiceError> {
    if !valid_task_id(id) || !(1..=CHUNK_BYTES).contains(&max_bytes) {
        return Err(error("invalid_artifact_request"));
    }
    let index = load(runtime, task)?;
    let entry = entries(&index)
        .find(|entry| entry.artifact.artifact_id == id)
        .ok_or_else(|| {
            ServiceError::new("not_found", "Artifact not found for this account task")
        })?;
    artifacts::read_entry(
        runtime,
        &task.id,
        &task.output_dir.join("voices"),
        entry,
        offset,
        max_bytes,
    )
}

#[cfg(test)]
#[path = "voice_artifacts_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn writer_test_fixture(base: &Path, number: u64) -> (RuntimeContext, Task) {
    tests::fixture(base, "writer-synthetic", number)
}

#[cfg(test)]
pub(crate) fn writer_test_terminal(
    runtime: &RuntimeContext,
    task: &mut Task,
) -> Vec<(String, Vec<u8>)> {
    use base64::Engine;
    task.status = "failed".into();
    task.result = Some(TaskResult::RawVoices(
        finalize(runtime, task, &mut FinalizeControl::supervised_worker()).unwrap(),
    ));
    let Some(TaskResult::RawVoices(result)) = &task.result else {
        panic!("wrong result")
    };
    assert_eq!(result.exported, 1);
    assert_eq!(result.unproven, 1);
    assert_eq!(result.artifact_count, 2);
    assert!(!result.artifacts_complete);
    let page = list(runtime, task, 0, 100).unwrap();
    assert_eq!(page.total, 2);
    page.items
        .into_iter()
        .map(|artifact| {
            let data = read(runtime, task, &artifact.artifact_id, 0, CHUNK_BYTES).unwrap();
            (
                artifact.name,
                base64::engine::general_purpose::STANDARD
                    .decode(data.data_base64)
                    .unwrap(),
            )
        })
        .collect()
}
