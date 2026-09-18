//! Immutable plan CSVs and per-chat apply checkpoints in the existing task journal.
use super::{
    artifact_file::{error, relative, Reader},
    artifacts::{self, Entry, FinalizeControl},
};
use crate::{
    application::{
        chat_export_plan::{render_plan_csv, PlanRow},
        chat_plan_selection::Plan,
    },
    runtime::RuntimeContext,
    service::{
        chat_plan::*,
        config_pin::ConfigPin,
        protocol::{valid_task_id, Kind, ServiceError, Submission, Task},
        task_artifacts::*,
    },
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    version: u32,
    runtime_id: String,
    task_id: String,
    request: Submission,
    request_sha256: String,
    config_sha256: String,
    result: TaskResult,
    rows: Vec<PlanRow>,
    entries: Vec<Entry>,
    chunks_left: usize,
}
pub(crate) struct ResolvedPlan {
    pub rows: Vec<PlanRow>,
    pub csv: Vec<u8>,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
}
pub(crate) fn is_plan(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::ChatPlan | Kind::ChatPlanReview | Kind::ChatPlanApply
    )
}
pub(crate) fn reference(request: &Submission) -> Option<&PlanRef> {
    match request.kind {
        Kind::ChatPlanReview => request
            .options
            .chat_plan_review
            .as_ref()
            .map(|r| &r.plan_ref),
        Kind::ChatPlanApply => request
            .options
            .chat_plan_apply
            .as_ref()
            .map(|r| &r.plan_ref),
        _ => None,
    }
}
fn signature(request: &Submission) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request)?)
    ))
}
pub(crate) fn root(runtime: &RuntimeContext, id: &str) -> Result<PathBuf> {
    ensure!(valid_task_id(id), "Invalid plan task identity");
    Ok(runtime.root.join("web-output").join(&runtime.id).join(id))
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
    let request = Submission {
        kind: task.kind,
        options: task.options.clone(),
    };
    crate::service::plan::validate(&request, &Default::default())?;
    let result = if let Some(apply) = &request.options.chat_plan_apply {
        TaskResult::PlanApply(ApplyResult {
            version: 1,
            scope: "chat_plan_apply".into(),
            finalized: false,
            outcome: None,
            plan_ref: apply.plan_ref.clone(),
            plan_mode: apply.plan_mode,
            dry_run: apply.dry_run,
            selected_count: 0,
            published_count: 0,
            failed_count: 0,
            messages: 0,
            artifact_count: 0,
            artifacts_complete: false,
            diagnostics: Vec::new(),
        })
    } else {
        let (start_ts, end_ts) = request
            .options
            .chat_plan
            .as_ref()
            .map(Request::resolved_window)
            .transpose()?
            .unwrap_or_default();
        TaskResult::Plan(PlanResult {
            version: 1,
            scope: "chat_plan".into(),
            finalized: false,
            outcome: None,
            published_plan_ref: None,
            parent_ref: reference(&request).cloned(),
            row_count: 0,
            partial_rows: 0,
            start_ts,
            end_ts,
            source_kind: "runtime_snapshot".into(),
            artifact_count: 0,
            artifacts_complete: false,
            diagnostics: Vec::new(),
        })
    };
    save(
        runtime,
        &Publication {
            version: 1,
            runtime_id: runtime.id.clone(),
            task_id: task.id.clone(),
            request_sha256: signature(&request)?,
            request,
            config_sha256: ConfigPin::new(runtime)?.fingerprint()?,
            result,
            rows: Vec::new(),
            entries: Vec::new(),
            chunks_left: 200_000,
        },
    )
}
fn load_bound(runtime: &RuntimeContext, id: &str) -> Result<Publication> {
    let index: Publication = artifacts::json(
        &artifacts::private_dir(runtime, id)?.join("artifact-index.json"),
        MAX_INDEX_BYTES,
    )?;
    ensure!(
        index.version == 1
            && index.runtime_id == runtime.id
            && index.task_id == id
            && is_plan(index.request.kind)
            && index.request_sha256 == signature(&index.request)?
            && index.config_sha256 == ConfigPin::new(runtime)?.fingerprint()?
            && index.result.validate(index.request.kind)
            && index.rows.len() <= MAX_ROWS
            && index.result.artifact_count() == index.entries.len() as u64
            && index.chunks_left <= 200_000,
        "Invalid plan publication binding"
    );
    crate::service::plan::validate(&index.request, &Default::default())?;
    let mut ids = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for entry in &index.entries {
        let path = relative(&entry.relative)?;
        ensure!(
            ids.insert(&entry.artifact.artifact_id)
                && paths.insert(&entry.relative)
                && valid_task_id(&entry.artifact.artifact_id)
                && valid_task_id(&entry.artifact.sha256)
                && entry.identity.size == entry.artifact.size
                && entry.chunks.len() as u64 == entry.artifact.size.div_ceil(CHUNK_BYTES as u64)
                && entry.chunks.iter().all(|h| valid_task_id(h)),
            "Invalid plan artifact"
        );
        ensure!(
            path.file_name().and_then(|s| s.to_str()) == Some(entry.artifact.name.as_str()),
            "Plan artifact name mismatch"
        );
        if index.request.kind == Kind::ChatPlanApply {
            ensure!(
                path.components().count() == 2
                    && path.starts_with("archive")
                    && path.extension().is_some_and(|e| e == "json")
                    && entry.artifact.role == "chat_document"
                    && entry.artifact.media_type == "application/json",
                "Invalid apply artifact path"
            );
        } else {
            ensure!(
                entry.relative == "plan.csv"
                    && entry.artifact.name == "plan.csv"
                    && entry.artifact.role == "chat_plan_csv"
                    && entry.artifact.media_type == "text/csv"
                    && entry.artifact.size <= MAX_PLAN_BYTES as u64,
                "Invalid plan artifact path"
            );
        }
    }
    match &index.result {
        TaskResult::Plan(result) => {
            ensure!(
                result.parent_ref.as_ref() == reference(&index.request),
                "Plan parent mismatch"
            );
            if let Some(request) = &index.request.options.chat_plan {
                ensure!(
                    request.resolved_window()? == (result.start_ts, result.end_ts),
                    "Plan range mismatch"
                );
            }
            ensure!(
                result.row_count == index.rows.len() as u64,
                "Plan row count mismatch"
            );
            if let Some(reference) = &result.published_plan_ref {
                let entry = index.entries.first().context("Missing plan entry")?;
                ensure!(
                    reference.task_id == id
                        && reference.artifact_id == entry.artifact.artifact_id
                        && reference.sha256 == entry.artifact.sha256,
                    "Published reference mismatch"
                );
            }
        }
        TaskResult::PlanApply(result) => {
            let request = index
                .request
                .options
                .chat_plan_apply
                .as_ref()
                .context("Missing apply request")?;
            ensure!(
                result.plan_ref == request.plan_ref
                    && result.plan_mode == request.plan_mode
                    && result.dry_run == request.dry_run
                    && index.rows.is_empty(),
                "Apply request mismatch"
            );
        }
        _ => anyhow::bail!("Invalid plan result scope"),
    }
    Ok(index)
}
fn load(runtime: &RuntimeContext, task: &Task) -> Result<Publication> {
    ensure!(
        task.terminal() && task.output_dir == root(runtime, &task.id)?,
        "Plan task unavailable"
    );
    let index = load_bound(runtime, &task.id)?;
    ensure!(
        index.request_sha256
            == signature(&Submission {
                kind: task.kind,
                options: task.options.clone()
            })?
            && serde_json::to_value(task.result.as_ref())? == serde_json::to_value(&index.result)?,
        "Plan task binding changed"
    );
    Ok(index)
}
pub(crate) fn resolve(
    runtime: &RuntimeContext,
    task: &Task,
    reference: &PlanRef,
) -> Result<ResolvedPlan, ServiceError> {
    reference
        .validate()
        .map_err(|_| error("plan_ref_unavailable"))?;
    if task.id != reference.task_id || !matches!(task.kind, Kind::ChatPlan | Kind::ChatPlanReview) {
        return Err(error("plan_ref_unavailable"));
    }
    let index = load(runtime, task).map_err(|_| error("plan_ref_unavailable"))?;
    let TaskResult::Plan(result) = index.result else {
        return Err(error("plan_ref_unavailable"));
    };
    if !result.finalized || result.published_plan_ref.as_ref() != Some(reference) {
        return Err(error("plan_ref_changed"));
    }
    let entry = index
        .entries
        .first()
        .ok_or_else(|| error("plan_ref_unavailable"))?;
    let mut reader = Reader::open(&task.output_dir.join("plan.csv"))?;
    if reader.identity != entry.identity {
        return Err(error("plan_ref_changed"));
    }
    let csv = reader.bounded(MAX_PLAN_BYTES as u64)?;
    if format!("{:x}", Sha256::digest(&csv)) != reference.sha256
        || render_plan_csv(&index.rows).map_err(|_| error("plan_ref_changed"))? != csv
    {
        return Err(error("plan_ref_changed"));
    }
    reader.verify()?;
    Ok(ResolvedPlan {
        rows: index.rows,
        csv,
        start_ts: result.start_ts,
        end_ts: result.end_ts,
    })
}
pub(crate) fn validate_changes(rows: &[PlanRow], request: &ReviewRequest) -> Result<()> {
    request.validate()?;
    let known: std::collections::HashSet<_> = rows.iter().map(|r| r.username.as_str()).collect();
    ensure!(
        request
            .changes
            .iter()
            .all(|c| known.contains(c.username.as_str())),
        "Unknown plan username"
    );
    Ok(())
}
pub(crate) fn publish_plan(
    runtime: &RuntimeContext,
    id: &str,
    rows: Vec<PlanRow>,
    window: (Option<i64>, Option<i64>),
) -> Result<()> {
    let pin = ConfigPin::new(runtime)?;
    let mut index = load_bound(runtime, id)?;
    ensure!(
        index.entries.is_empty() && rows.len() <= MAX_ROWS,
        "Plan already published or too large"
    );
    let csv = render_plan_csv(&rows)?;
    ensure!(csv.len() <= MAX_PLAN_BYTES, "Plan CSV exceeds limit");
    // Bound the private projection before publishing any CSV.
    index.rows = rows;
    if let TaskResult::Plan(result) = &mut index.result {
        result.row_count = index.rows.len() as u64;
        result.partial_rows = index.rows.iter().filter(|r| r.size_status != "ok").count() as u64;
        result.start_ts = window.0;
        result.end_ts = window.1;
    } else {
        anyhow::bail!("Wrong plan result kind");
    }
    ensure!(
        serde_json::to_vec(&index)?.len() as u64 + 8192 <= MAX_INDEX_BYTES,
        "Plan projection exceeds limit"
    );
    let path = root(runtime, id)?.join("plan.csv");
    crate::infrastructure::publication::ExportTarget::new_file(
        &path,
        &crate::infrastructure::publication::export_protected(runtime),
    )?
    .write_bytes_checked(&csv, || pin.verify(runtime))?;
    let expected = format!("{:x}", Sha256::digest(&csv));
    let registered = artifacts::register(
        &path,
        "plan.csv".into(),
        "plan.csv",
        Some(&expected),
        &mut index.chunks_left,
        &mut FinalizeControl::supervised_worker(),
    );
    let mut entry = match registered {
        Ok(entry) => entry,
        Err(e) => {
            index.rows.clear();
            if let TaskResult::Plan(r) = &mut index.result {
                r.row_count = 0;
                r.partial_rows = 0;
            }
            save(runtime, &index)?;
            return Err(e);
        }
    };
    entry.artifact.role = "chat_plan_csv".into();
    entry.artifact.media_type = "text/csv".into();
    if let TaskResult::Plan(result) = &mut index.result {
        result.published_plan_ref = Some(PlanRef {
            task_id: id.into(),
            artifact_id: entry.artifact.artifact_id.clone(),
            sha256: expected,
        });
        result.artifact_count = 1;
        result.artifacts_complete = true;
        result.finalized = true;
        result.outcome = Some(if result.partial_rows == 0 {
            ExportOutcome::Success
        } else {
            ExportOutcome::Partial
        });
        if result.partial_rows > 0 {
            result.diagnostics.push(Diagnostic {
                code: "plan_partial".into(),
                count: result.partial_rows,
            });
        }
    }
    index.entries.push(entry);
    pin.verify(runtime)?;
    save(runtime, &index)?;
    pin.verify(runtime)
}
pub(crate) fn apply_selected(runtime: &RuntimeContext, id: &str, count: usize) -> Result<()> {
    let mut index = load_bound(runtime, id)?;
    let TaskResult::PlanApply(result) = &mut index.result else {
        anyhow::bail!("Wrong apply result");
    };
    result.selected_count = count as u64;
    save(runtime, &index)
}
pub(crate) fn publish_chat(
    runtime: &RuntimeContext,
    id: &str,
    path: &Path,
    document: &serde_json::Value,
) -> Result<()> {
    let pin = ConfigPin::new(runtime)?;
    let mut index = load_bound(runtime, id)?;
    ensure!(
        index.entries.len() < MAX_ARTIFACTS,
        "Apply artifact limit exceeded"
    );
    let rel = path
        .strip_prefix(root(runtime, id)?)?
        .to_str()
        .context("Invalid apply path")?
        .replace('\\', "/");
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("Invalid apply filename")?;
    ensure!(
        !index.entries.iter().any(|entry| entry.relative == rel),
        "Apply file already registered"
    );
    let bytes = serde_json::to_vec_pretty(document)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let registered = artifacts::register(
        path,
        rel,
        name,
        Some(&hash),
        &mut index.chunks_left,
        &mut FinalizeControl::supervised_worker(),
    );
    let mut entry = match registered {
        Ok(entry) => entry,
        Err(e) => {
            save(runtime, &index)?;
            return Err(e);
        }
    };
    while index
        .entries
        .iter()
        .any(|known| known.artifact.artifact_id == entry.artifact.artifact_id)
    {
        entry.artifact.artifact_id = crate::service::transport::random_secret()?.to_string();
    }
    let previous_result = index.result.clone();
    let TaskResult::PlanApply(result) = &mut index.result else {
        anyhow::bail!("Wrong apply result");
    };
    ensure!(!result.dry_run, "Dry run cannot publish");
    result.published_count += 1;
    result.artifact_count += 1;
    result.messages = result
        .messages
        .checked_add(
            document["messages"]
                .as_array()
                .context("Missing messages")?
                .len() as u64,
        )
        .context("Message count overflow")?;
    ensure!(result.validate(), "Invalid apply result");
    index.entries.push(entry);
    if serde_json::to_vec(&index)?.len() as u64 > MAX_INDEX_BYTES {
        index.entries.pop();
        index.result = previous_result;
        save(runtime, &index)?;
        return Err(error("artifact_limit_exceeded").into());
    }
    pin.verify(runtime)?;
    save(runtime, &index)?;
    pin.verify(runtime)
}
pub(crate) fn finish_apply(runtime: &RuntimeContext, id: &str, failed: u64) -> Result<()> {
    let mut index = load_bound(runtime, id)?;
    let TaskResult::PlanApply(result) = &mut index.result else {
        anyhow::bail!("Wrong apply result");
    };
    result.failed_count = failed;
    result.finalized = true;
    result.artifacts_complete = true;
    result.outcome = Some(if failed == 0 {
        ExportOutcome::Success
    } else if result.published_count > 0 {
        ExportOutcome::Partial
    } else {
        ExportOutcome::Failure
    });
    if failed > 0 {
        result.diagnostics.push(Diagnostic {
            code: "chat_export_failed".into(),
            count: failed,
        });
    }
    ensure!(result.validate(), "Invalid apply result");
    save(runtime, &index)
}
pub(super) fn finalize(
    runtime: &RuntimeContext,
    task: &Task,
    control: &mut FinalizeControl,
) -> Result<TaskResult> {
    let mut index = load_bound(runtime, &task.id)?;
    ensure!(
        index.request_sha256
            == signature(&Submission {
                kind: task.kind,
                options: task.options.clone()
            })?,
        "Plan request mismatch"
    );
    let check = (|| -> Result<()> {
        control.check()?;
        for entry in &index.entries {
            control.check()?;
            let reader = Reader::open(&root(runtime, &task.id)?.join(relative(&entry.relative)?))?;
            ensure!(reader.identity == entry.identity, "Plan artifact changed");
            reader.verify()?;
        }
        control.check()?;
        Ok(())
    })();
    let code =
        check.err().map(
            |e| match e.downcast_ref::<ServiceError>().map(|e| e.code.as_str()) {
                Some("artifact_finalization_cancelled") => "export_interrupted",
                Some("artifact_finalization_timeout") => "artifact_finalization_timeout",
                _ => "artifact_changed",
            },
        );
    let (finalized, complete, outcome, diagnostics) = match &mut index.result {
        TaskResult::Plan(r) => (
            r.finalized,
            &mut r.artifacts_complete,
            &mut r.outcome,
            &mut r.diagnostics,
        ),
        TaskResult::PlanApply(r) => (
            r.finalized,
            &mut r.artifacts_complete,
            &mut r.outcome,
            &mut r.diagnostics,
        ),
        _ => anyhow::bail!("Wrong plan result"),
    };
    if let Some(code) = code.or((!finalized).then_some("plan_export_failed")) {
        *complete = false;
        *outcome = Some(if index.entries.is_empty() {
            ExportOutcome::Failure
        } else {
            ExportOutcome::Partial
        });
        if !diagnostics.iter().any(|d| d.code == code) {
            diagnostics.push(Diagnostic {
                code: code.into(),
                count: 1,
            });
        }
    }
    ensure!(
        index.result.validate(task.kind),
        "Invalid plan final result"
    );
    save(runtime, &index)?;
    Ok(index.result)
}
pub(super) fn list(
    runtime: &RuntimeContext,
    task: &Task,
    offset: u64,
    limit: u32,
) -> Result<ArtifactsPage, ServiceError> {
    let index = load(runtime, task).map_err(|_| error("result_unavailable"))?;
    let total = index.entries.len() as u64;
    if offset > total || !(1..=MAX_LIST_ITEMS).contains(&limit) {
        return Err(error("invalid_artifact_request"));
    }
    let items: Vec<_> = index
        .entries
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|e| e.artifact)
        .collect();
    let next = offset + items.len() as u64;
    Ok(ArtifactsPage {
        version: 1,
        task_id: task.id.clone(),
        scope: if task.kind == Kind::ChatPlanApply {
            "chat_plan_apply"
        } else {
            "chat_plan"
        }
        .into(),
        items,
        offset,
        next_offset: (next < total).then_some(next),
        total,
        complete: index.result.artifacts_complete(),
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
    let index = load(runtime, task).map_err(|_| error("result_unavailable"))?;
    let entry = index
        .entries
        .iter()
        .find(|e| e.artifact.artifact_id == id)
        .ok_or_else(|| error("not_found"))?;
    artifacts::read_entry(
        runtime,
        &task.id,
        &task.output_dir,
        entry,
        offset,
        max_bytes,
    )
}
pub(super) fn read_plan(
    runtime: &RuntimeContext,
    task: &Task,
    reference: PlanRef,
    mode: Mode,
    offset: u64,
    limit: u32,
) -> Result<ReadReply, ServiceError> {
    if !(1..=100).contains(&limit) {
        return Err(error("invalid_page"));
    }
    let resolved = resolve(runtime, task, &reference)?;
    let skip = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(resolved.rows.len());
    let selected = Plan::read(resolved.csv.as_slice(), mode)
        .and_then(|p| {
            p.select(resolved.rows.iter().map(|r| r.username.as_str()))
                .map(<[String]>::to_vec)
        })
        .map_err(|_| error("plan_selection_invalid"))?;
    let selected_count = selected.len() as u64;
    let selected: std::collections::HashSet<_> = selected.into_iter().collect();
    let total = resolved.rows.len() as u64;
    let rows: Vec<_> = resolved
        .rows
        .into_iter()
        .skip(skip)
        .take(limit as usize)
        .map(|r| Row {
            selected: selected.contains(&r.username),
            export: r.export,
            index: r.index,
            username: r.username,
            chat_name: r.chat_name,
            chat_type: r.chat_type,
            message_count: r.message_count,
            first_time: r.first_time,
            last_time: r.last_time,
            attachment_estimated_bytes: r.attachment_estimated_bytes,
            attachment_scanned_bytes: r.attachment_scanned_bytes,
            total_estimated_bytes: r.total_estimated_bytes,
            size_status: r.size_status,
        })
        .collect();
    let next = offset
        .checked_add(rows.len() as u64)
        .ok_or_else(|| error("invalid_page"))?;
    let reply = ReadReply {
        version: 1,
        plan_ref: reference,
        plan_mode: mode,
        rows,
        offset,
        next_offset: (next < total).then_some(next),
        total,
        selected_count,
        start_ts: resolved.start_ts,
        end_ts: resolved.end_ts,
        source_kind: "runtime_snapshot".into(),
    };
    if serde_json::to_vec(&reply)
        .map_err(|_| error("result_unavailable"))?
        .len()
        > MAX_READ_BYTES
    {
        return Err(error("result_limit"));
    }
    Ok(reply)
}

#[cfg(test)]
#[path = "plan_artifacts_tests.rs"]
mod tests;
