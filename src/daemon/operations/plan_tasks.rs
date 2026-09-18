//! Narrow task orchestration over existing planning, selection and export operations.
use crate::{
    application::{chat_export_plan, chat_plan_selection::Plan},
    business::chat_plan::{PlanChat, TimeRange},
    daemon::tasks::plan_artifacts,
    message::export::Target,
    runtime::RuntimeContext,
    service::{
        chat_plan::*,
        config_pin::ConfigPin,
        protocol::{Call, Task},
    },
};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

pub(crate) fn catalog(runtime: &RuntimeContext) -> Result<Vec<Target>> {
    let response =
        crate::service::query_client::send_for(runtime, crate::ipc::Request::ExportChatList)?;
    response.require_success()?;
    let targets: Vec<Target> = serde_json::from_value(
        response
            .data
            .get("chats")
            .context("Missing chat catalog")?
            .clone(),
    )?;
    let mut known = std::collections::HashSet::new();
    ensure!(
        targets
            .iter()
            .all(|t| !t.username.is_empty() && known.insert(&t.username)),
        "Ambiguous account chat catalog"
    );
    Ok(targets)
}
pub(crate) fn selected(runtime: &RuntimeContext, csv: &[u8], mode: Mode) -> Result<Vec<Target>> {
    let targets = catalog(runtime)?;
    let plan = Plan::read(csv, mode)?;
    let selected = plan.select(targets.iter().map(|t| t.username.as_str()))?;
    let mut lookup: std::collections::HashMap<_, _> = targets
        .into_iter()
        .map(|t| (t.username.clone(), t))
        .collect();
    selected
        .iter()
        .map(|u| lookup.remove(u).context("Selected chat missing"))
        .collect()
}
pub(crate) fn selection_hash(targets: &[Target]) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(
            &targets.iter().map(|t| &t.username).collect::<Vec<_>>()
        )?)
    ))
}
fn source_task(runtime: &RuntimeContext, reference: &PlanRef) -> Result<Task> {
    reference.validate()?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let data = executor.block_on(crate::service::client::request(
        runtime,
        Call::Get {
            id: reference.task_id.clone(),
        },
    ))?;
    Ok(serde_json::from_value(data)?)
}
pub(crate) fn generate(runtime: &RuntimeContext, id: &str, request: Request) -> Result<()> {
    request.validate()?;
    let pin = ConfigPin::new(runtime)?;
    let chats = plan_chats(runtime, &request)?;
    let keys = crate::service::worker_keys::database_keys(runtime)?
        .context("Missing task database read capability")?;
    let snapshot = super::media_snapshot::prepare_snapshot(
        runtime,
        super::media_snapshot::DatabaseMaterials::new(keys.0.clone()),
    )?;
    ensure!(
        snapshot.account_id() == runtime.id,
        "Plan snapshot account mismatch"
    );
    let (root, databases) = snapshot.planning_inputs()?;
    let window = request.resolved_window()?;
    let range = TimeRange {
        start: window.0,
        end: window.1,
    };
    let rows = match request.size_mode {
        SizeMode::Estimate => {
            chat_export_plan::collect_plan(root, &databases, &chats, range, SizeMode::Estimate)?
        }
        SizeMode::Scan => {
            let source = if runtime
                .config
                .db_dir
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("db_storage"))
            {
                runtime
                    .config
                    .db_dir
                    .parent()
                    .context("Account source has no parent")?
                    .to_owned()
            } else {
                runtime.config.db_dir.clone()
            };
            chat_export_plan::collect_plan_with_scan(
                root,
                &databases,
                &chats,
                range,
                &chat_export_plan::ScanOptions {
                    source_dir: Some(source),
                    media_dir: None,
                    workers: request.threads as usize,
                },
            )?
        }
    };
    pin.verify(runtime)?;
    plan_artifacts::publish_plan(runtime, id, rows, window)?;
    pin.verify(runtime)
}

pub(crate) fn plan_chats(runtime: &RuntimeContext, request: &Request) -> Result<Vec<PlanChat>> {
    request.validate()?;
    let targets = catalog(runtime)?;
    let known: std::collections::HashSet<_> = targets.iter().map(|t| t.username.as_str()).collect();
    if let Some(users) = &request.users {
        ensure!(
            users.iter().all(|u| known.contains(u.as_str())),
            "Unknown plan username"
        );
    }
    let chats: Vec<_> = targets
        .into_iter()
        .enumerate()
        .filter(|(_, t)| {
            request
                .users
                .as_ref()
                .is_none_or(|users| users.contains(&t.username))
                && !request.exclude_users.contains(&t.username)
        })
        .map(|(index, t)| PlanChat {
            index: index + 1,
            username: t.username,
            chat_name: t.chat,
            chat_type: if t.is_group { "group" } else { "single" }.into(),
        })
        .collect();
    ensure!(chats.len() <= MAX_ROWS, "Plan row limit exceeded");
    Ok(chats)
}
pub(crate) fn review(runtime: &RuntimeContext, id: &str, request: ReviewRequest) -> Result<()> {
    let pin = ConfigPin::new(runtime)?;
    let parent = source_task(runtime, &request.plan_ref)?;
    let mut resolved = plan_artifacts::resolve(runtime, &parent, &request.plan_ref)?;
    plan_artifacts::validate_changes(&resolved.rows, &request)?;
    for change in &request.changes {
        resolved
            .rows
            .iter_mut()
            .find(|r| r.username == change.username)
            .context("Unknown plan username")?
            .export
            .clone_from(&change.export);
    }
    pin.verify(runtime)?;
    plan_artifacts::publish_plan(
        runtime,
        id,
        resolved.rows,
        (resolved.start_ts, resolved.end_ts),
    )?;
    pin.verify(runtime)
}
pub(crate) fn apply(
    runtime: &RuntimeContext,
    id: &str,
    request: ApplyRequest,
    expected_selection: &str,
) -> Result<()> {
    let pin = ConfigPin::new(runtime)?;
    request.validate()?;
    let parent = source_task(runtime, &request.plan_ref)?;
    let resolved = plan_artifacts::resolve(runtime, &parent, &request.plan_ref)?;
    let targets = selected(runtime, &resolved.csv, request.plan_mode)?;
    ensure!(
        selection_hash(&targets)? == expected_selection,
        "Plan selection changed since submission"
    );
    plan_artifacts::apply_selected(runtime, id, targets.len())?;
    if request.dry_run || targets.is_empty() {
        pin.verify(runtime)?;
        return plan_artifacts::finish_apply(runtime, id, 0);
    }
    let output = plan_artifacts::root(runtime, id)?.join("archive");
    let mut published = |path: &std::path::Path, document: &serde_json::Value| {
        pin.verify(runtime)?;
        plan_artifacts::publish_chat(runtime, id, path, document)
    };
    let report = super::export_chats::export_selected_for(
        runtime,
        &output,
        targets,
        (resolved.start_ts, resolved.end_ts),
        &mut published,
    )?;
    let failed = report["failures"]
        .as_array()
        .context("Missing export failures")?
        .len() as u64;
    pin.verify(runtime)?;
    plan_artifacts::finish_apply(runtime, id, failed)?;
    ensure!(
        failed == 0,
        "One or more selected chats could not be exported"
    );
    Ok(())
}
