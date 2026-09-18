//! One history file, committed by its supervised worker, never discovered by scanning.
use super::{
    artifact_file::{error, Reader},
    artifacts::{self, Entry, FinalizeControl},
};
use crate::{
    runtime::RuntimeContext,
    service::{
        history_export::{HistoryExportResult, HistoryQuerySummary, Request, MAX_OUTPUT_BYTES},
        protocol::{valid_task_id, Kind, Options, ServiceError, Submission, Task},
        task_artifacts::*,
    },
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const INDEX_LIMIT: u64 = 128 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryPublication {
    version: u32,
    runtime_id: String,
    task_id: String,
    request_sha256: String,
    result: HistoryExportResult,
    entry: Option<Entry>,
}

fn signature(request: &Request) -> Result<String> {
    let submission = Submission {
        kind: Kind::ExportHistory,
        options: Options {
            history_export: Some(request.clone()),
            ..Default::default()
        },
    };
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&submission)?)
    ))
}

pub(crate) fn output_path(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
) -> Result<PathBuf> {
    ensure!(valid_task_id(id), "Invalid history task identity");
    Ok(runtime
        .root
        .join("web-output")
        .join(&runtime.id)
        .join(id)
        .join("history")
        .join(format!("history.{}", request.format.extension())))
}

fn save(runtime: &RuntimeContext, publication: &HistoryPublication) -> Result<()> {
    artifacts::persist(
        runtime,
        &publication.task_id,
        "artifact-index.json",
        publication,
        INDEX_LIMIT,
    )
}

pub(super) fn start(runtime: &RuntimeContext, task: &Task) -> Result<()> {
    let request = selection(task)?;
    save(
        runtime,
        &HistoryPublication {
            version: 1,
            runtime_id: runtime.id.clone(),
            task_id: task.id.clone(),
            request_sha256: signature(request)?,
            result: HistoryExportResult::empty(request.format),
            entry: None,
        },
    )
}

fn selection(task: &Task) -> Result<&Request> {
    ensure!(task.kind == Kind::ExportHistory, "Wrong history task kind");
    crate::service::plan::validate(
        &Submission {
            kind: task.kind,
            options: task.options.clone(),
        },
        &crate::service::settings::Settings::default(),
    )?;
    task.options
        .history_export
        .as_ref()
        .ok_or_else(|| error("result_unavailable").into())
}

fn load_bound(runtime: &RuntimeContext, id: &str, request: &Request) -> Result<HistoryPublication> {
    let index: HistoryPublication = artifacts::json(
        &artifacts::private_dir(runtime, id)?.join("artifact-index.json"),
        INDEX_LIMIT,
    )?;
    ensure!(
        index.version == 1
            && index.runtime_id == runtime.id
            && index.task_id == id
            && index.request_sha256 == signature(request)?
            && index.result.validate()
            && index.result.format == request.format
            && index.result.artifact_count == u64::from(index.entry.is_some()),
        "Invalid history publication"
    );
    if let Some(query) = &index.result.query {
        ensure!(query.limit == request.limit, "History query limit mismatch");
    }
    if let Some(entry) = &index.entry {
        let expected = format!("history.{}", request.format.extension());
        ensure!(
            entry.relative == expected
                && entry.artifact.name == expected
                && entry.artifact.role == "chat_document"
                && entry.artifact.media_type == request.format.media_type()
                && valid_task_id(&entry.artifact.artifact_id)
                && valid_task_id(&entry.artifact.sha256)
                && entry.identity.size == entry.artifact.size
                && entry.artifact.size <= MAX_OUTPUT_BYTES as u64
                && entry.chunks.len() as u64 == entry.artifact.size.div_ceil(CHUNK_BYTES as u64)
                && entry.chunks.iter().all(|h| valid_task_id(h)),
            "Invalid history artifact"
        );
    }
    Ok(index)
}

pub(crate) fn published(
    runtime: &RuntimeContext,
    id: &str,
    request: &Request,
    query: HistoryQuerySummary,
    warning_count: u64,
    expected_sha256: &str,
) -> Result<()> {
    let pin = crate::service::config_pin::ConfigPin::new(runtime)?;
    let mut index = load_bound(runtime, id, request)?;
    ensure!(
        index.entry.is_none() && !index.result.finalized,
        "History already published"
    );
    let path = output_path(runtime, id, request)?;
    let name = format!("history.{}", request.format.extension());
    let mut chunks_left = MAX_OUTPUT_BYTES.div_ceil(CHUNK_BYTES as usize);
    let mut entry = artifacts::register(
        &path,
        name.clone(),
        &name,
        Some(expected_sha256),
        &mut chunks_left,
        &mut FinalizeControl::supervised_worker(),
    )?;
    entry.artifact.role = "chat_document".into();
    entry.artifact.media_type = request.format.media_type().into();
    index.entry = Some(entry);
    index.result.query = Some(query);
    index.result.finalized = true;
    index.result.artifact_count = 1;
    index.result.artifacts_complete = true;
    index.result.outcome = Some(if warning_count == 0 {
        ExportOutcome::Success
    } else {
        ExportOutcome::Partial
    });
    if warning_count > 0 {
        index.result.diagnostics.push(Diagnostic {
            code: "history_query_warning".into(),
            count: warning_count,
        });
    }
    ensure!(index.result.validate(), "Invalid history result");
    pin.verify(runtime)?;
    save(runtime, &index)?;
    pin.verify(runtime)
}

pub(super) fn finalize(
    runtime: &RuntimeContext,
    task: &Task,
    control: &mut FinalizeControl,
) -> Result<HistoryExportResult> {
    let request = selection(task)?;
    let mut index = load_bound(runtime, &task.id, request)?;
    let verification = (|| -> Result<()> {
        control.check()?;
        if let Some(entry) = &index.entry {
            let reader = Reader::open(&output_path(runtime, &task.id, request)?)?;
            ensure!(
                reader.identity == entry.identity,
                "History artifact changed"
            );
            reader.verify()?;
        }
        control.check()?;
        Ok(())
    })();
    if let Err(e) = verification {
        let code = match e.downcast_ref::<ServiceError>().map(|e| e.code.as_str()) {
            Some("artifact_finalization_cancelled") => "export_interrupted",
            Some("artifact_finalization_timeout") => "artifact_finalization_timeout",
            Some("artifact_unsafe") => "artifact_unsafe",
            Some("artifact_unavailable") => "artifact_unavailable",
            _ => "artifact_changed",
        };
        index.result.artifacts_complete = false;
        index.result.outcome = index.entry.as_ref().map(|_| ExportOutcome::Partial);
        if !index.result.diagnostics.iter().any(|d| d.code == code) {
            index.result.diagnostics.push(Diagnostic {
                code: code.into(),
                count: 1,
            });
        }
    } else if index.entry.is_none() {
        index.result.outcome = Some(ExportOutcome::Failure);
        if !index
            .result
            .diagnostics
            .iter()
            .any(|d| d.code == "history_export_failed")
        {
            index.result.diagnostics.push(Diagnostic {
                code: "history_export_failed".into(),
                count: 1,
            });
        }
    }
    ensure!(index.result.validate(), "Invalid history result");
    save(runtime, &index)?;
    Ok(index.result)
}

fn load(runtime: &RuntimeContext, task: &Task) -> Result<HistoryPublication, ServiceError> {
    let request = selection(task).map_err(|_| error("result_unavailable"))?;
    let expected =
        output_path(runtime, &task.id, request).map_err(|_| error("result_unavailable"))?;
    if task
        .output_dir
        .join("history")
        .join(format!("history.{}", request.format.extension()))
        != expected
    {
        return Err(error("result_unavailable"));
    }
    let index = load_bound(runtime, &task.id, request).map_err(|_| error("result_unavailable"))?;
    match &task.result {
        Some(TaskResult::ChatHistory(result))
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
    if !(1..=MAX_LIST_ITEMS).contains(&limit) || offset > total {
        return Err(error("invalid_artifact_request"));
    }
    let items = index
        .entry
        .into_iter()
        .skip(offset as usize)
        .map(|e| e.artifact)
        .collect();
    Ok(ArtifactsPage {
        version: 1,
        task_id: task.id.clone(),
        scope: "chat_history".into(),
        items,
        offset,
        next_offset: None,
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
    let entry = index
        .entry
        .as_ref()
        .filter(|e| e.artifact.artifact_id == id)
        .ok_or_else(|| {
            ServiceError::new("not_found", "Artifact not found for this account task")
        })?;
    artifacts::read_entry(
        runtime,
        &task.id,
        &task.output_dir.join("history"),
        entry,
        offset,
        max_bytes,
    )
}

#[cfg(test)]
#[path = "history_artifacts_tests.rs"]
mod tests;
