use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::history::{parse_time, parse_time_end};
use super::output::print_value;
use crate::adapters::wechat::media::{
    voice_catalog::MediaShard,
    voice_export::{self, Catalog, VoiceRow},
};
use crate::business::voice_export::{self as domain, Selection};
use crate::daemon::query::{chat_type_of, Names};
use crate::infrastructure::publication::ExportTarget;
use crate::runtime::RuntimeContext;

#[derive(Debug, Serialize)]
struct ExportedVoice {
    chat: String,
    chat_username: String,
    chat_type: String,
    timestamp: Option<i64>,
    time: Option<String>,
    local_id: Option<i64>,
    svr_id: Option<i64>,
    chat_name_id: i64,
    data_index: Option<String>,
    media_rowid: i64,
    relative_path: String,
    message_id: Option<String>,
    sender: Option<String>,
    duration_ms: Option<u64>,
    association: String,
    media_db: String,
    audio_file: String,
    evidence_file: String,
    audio_format: String,
    voice_data_bytes: usize,
    raw_had_0x02_prefix: bool,
    silk_header_ok: bool,
}

pub use crate::service::operation_requests::voices::Args;

struct TaskPublication<'a> {
    id: &'a str,
    request: &'a crate::service::voice_export::Request,
    pin: crate::service::config_pin::ConfigPin,
    material: [u8; 32],
    #[cfg(test)]
    synthetic_store: bool,
    #[cfg(test)]
    writer_fault: Option<WriterFault>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum WriterFault {
    SecondPublication,
    SecondRegistration,
}

fn material_digest(keys: &HashMap<String, String>) -> [u8; 32] {
    let mut entries = keys.iter().collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(source, _)| *source);
    let mut hash = Sha256::new();
    for (source, key) in entries {
        hash.update((source.len() as u64).to_le_bytes());
        hash.update(source.as_bytes());
        hash.update((key.len() as u64).to_le_bytes());
        hash.update(key.as_bytes());
    }
    hash.finalize().into()
}

impl TaskPublication<'_> {
    fn verify(&self, runtime: &RuntimeContext) -> Result<()> {
        self.pin.verify(runtime)?;
        #[cfg(test)]
        let keys = if self.synthetic_store {
            Some(crate::service::worker_keys::DatabaseKeys(
                crate::key_store::Store::for_runtime(runtime)?
                    .load()?
                    .database_keys(),
            ))
        } else {
            crate::service::worker_keys::database_keys(runtime)?
        };
        #[cfg(not(test))]
        let keys = crate::service::worker_keys::database_keys(runtime)?;
        let keys = keys.ok_or(crate::key_store::Error::Missing)?;
        anyhow::ensure!(
            material_digest(&keys.0) == self.material,
            "Voice material changed"
        );
        Ok(())
    }
}

pub(crate) fn export_task_for(
    runtime: &RuntimeContext,
    id: &str,
    request: crate::service::voice_export::Request,
    window: (Option<i64>, Option<i64>),
    output: &Path,
) -> Result<()> {
    anyhow::ensure!(
        request.resolved_window()? == window
            && output == crate::daemon::tasks::voice_artifacts::root(runtime, id)?,
        "Invalid voice task binding"
    );
    let keys = crate::service::worker_keys::database_keys(runtime)?
        .ok_or(crate::key_store::Error::Missing)?;
    let task = TaskPublication {
        id,
        request: &request,
        pin: crate::service::config_pin::ConfigPin::new(runtime)?,
        material: material_digest(&keys.0),
        #[cfg(test)]
        synthetic_store: false,
        #[cfg(test)]
        writer_fault: None,
    };
    drop(keys);
    let result = tokio::runtime::Runtime::new()?.block_on(export_voices(
        runtime,
        request.chat.clone(),
        output.to_owned(),
        Selection {
            since: window.0,
            until: window.1,
            limit: request.limit,
            offset: request.offset,
            ..Default::default()
        },
        false,
        Some(&task),
    ));
    let summary = match result {
        Ok(summary) => summary,
        Err(error) => {
            let budget = error.is::<voice_export::CatalogBudgetExceeded>()
                || error
                    .downcast_ref::<crate::service::protocol::ServiceError>()
                    .is_some_and(|error| error.code == "artifact_limit_exceeded");
            crate::daemon::tasks::voice_artifacts::failed(runtime, id, &request, budget)?;
            return Err(error);
        }
    };
    crate::ipc::outcome::BusinessOutcome::from_counts(
        summary["exported"]
            .as_u64()
            .context("Missing voice export count")?,
        summary["incomplete_items"]
            .as_u64()
            .context("Missing voice incomplete count")?,
    )
    .require_success()?;
    Ok(())
}

pub fn cmd_voices(args: Args) -> Result<()> {
    let Args {
        chat,
        output,
        limit,
        offset,
        since,
        until,
        overwrite,
        json: json_output,
    } = args;
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;
    let runtime = RuntimeContext::load()?;
    if let Some(expected) = std::env::var_os("WX_CLI_EXPECTED_RUNTIME") {
        anyhow::ensure!(
            expected.to_str() == Some(runtime.id.as_str()),
            "Account changed before voice export"
        );
    }
    let _config_pin = crate::service::config_pin::ConfigPin::new(&runtime)?;
    let rt = tokio::runtime::Runtime::new().context("创建运行时失败")?;
    let summary = rt.block_on(async {
        export_voices(
            &runtime,
            chat,
            PathBuf::from(output),
            Selection {
                limit,
                offset,
                since: since_ts,
                until: until_ts,
                ..Default::default()
            },
            overwrite,
            None,
        )
        .await
    })?;
    print_value(&summary, &super::output::resolve(json_output))?;
    crate::ipc::outcome::BusinessOutcome::from_counts(
        summary["exported"].as_u64().unwrap_or(0),
        summary["incomplete_items"].as_u64().unwrap_or(0),
    )
    .require_success()?;
    Ok(())
}

async fn export_voices(
    runtime: &RuntimeContext,
    chat: Option<String>,
    out_dir: PathBuf,
    selection: Selection<'_>,
    overwrite: bool,
    task: Option<&TaskPublication<'_>>,
) -> Result<Value> {
    let out_dir = std::path::absolute(out_dir)?;
    crate::infrastructure::publication::validate_export_target(runtime, &out_dir)?;
    let mut all_keys = crate::service::worker_keys::database_keys(runtime)?
        .ok_or(crate::key_store::Error::Missing)?;
    // The cache indexes original key spellings; catalog identities use canonical sources.
    let mut lookup_keys = HashMap::new();
    for key in all_keys.0.keys() {
        let source = key.replace('\\', "/").to_ascii_lowercase();
        anyhow::ensure!(
            lookup_keys.insert(source, key.clone()).is_none(),
            "Ambiguous database source spelling"
        );
    }
    let media_paths = voice_export::media_database_paths(all_keys.0.keys());
    let message_paths: Vec<_> = all_keys
        .0
        .keys()
        .map(|key| key.replace('\\', "/").to_ascii_lowercase())
        .filter(|key| {
            key.strip_prefix("message/message_")
                .and_then(|key| key.strip_suffix(".db"))
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                })
        })
        .collect();
    if media_paths.is_empty() {
        bail!("密钥库里没有 message/media_*.db 的密钥，请先运行 wx init --force");
    }
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("创建输出目录失败: {}", out_dir.display()))?;
    let output_guard = crate::attachment::local_files::HostOutputGuard::new(&out_dir)?;
    let summary_path = out_dir.join("_voice_export_summary.json");
    let summary_target = if task.is_some() {
        capture_voice_target(runtime, &summary_path, false)?
    } else {
        ExportTarget::capture(runtime, &summary_path)?
    };
    let mut snapshot = super::media_snapshot::prepare_voice_snapshot(
        runtime,
        super::media_snapshot::DatabaseMaterials::new(std::mem::take(&mut all_keys.0)),
    )?;
    let empty_names = || Names {
        map: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    };
    let mut names = std::mem::replace(&mut snapshot.names, empty_names());
    if let Some(task) = task {
        task.verify(runtime)?;
    }

    let target_username = chat
        .as_deref()
        .map(|name| {
            domain::resolve_chat(
                name,
                names
                    .map
                    .iter()
                    .map(|(user, display)| (user.as_str(), display.as_str())),
            )
        })
        .transpose()?;

    let mut shards = Vec::new();
    let mut missing_shards = snapshot.missing_media.clone();
    let mut association_sources = Vec::new();
    for rel_key in media_paths {
        let Some(path) = source_path(&snapshot, &lookup_keys[&rel_key]) else {
            missing_shards.push(rel_key);
            continue;
        };
        shards.push(MediaShard {
            source: rel_key.clone(),
            path: path.clone(),
        });
        association_sources.push(crate::adapters::wechat::media::voice::DecryptedSource {
            source: rel_key,
            path,
        });
    }
    let mut missing_message_shards = snapshot.missing_messages.clone();
    for key in &message_paths {
        match source_path(&snapshot, &lookup_keys[key]) {
            Some(path) => {
                association_sources.push(crate::adapters::wechat::media::voice::DecryptedSource {
                    source: key.clone(),
                    path,
                })
            }
            None => missing_message_shards.push(key.clone()),
        }
    }
    missing_shards.sort();
    missing_shards.dedup();
    missing_message_shards.sort();
    missing_message_shards.dedup();
    let source = if task.is_some() {
        Catalog::open_bounded(shards, 32 * 1024 * 1024)?
    } else {
        Catalog::open(shards)?
    };
    let selected = domain::select(
        &source,
        &Selection {
            username: target_username.as_deref(),
            ..selection
        },
    );
    let scanned = selected.len();
    if let Some(task) = task {
        crate::daemon::tasks::voice_artifacts::selected(
            runtime,
            task.id,
            task.request,
            scanned,
            target_username.clone(),
        )?;
    }
    let mut exported = Vec::new();
    let mut manifest = Vec::new();
    let mut summary_budget = 32usize * 1024 * 1024;
    for entry in selected {
        let exported_before = exported.len();
        let mut item = source.manifest(entry.slot, &runtime.id)?;
        if item.status != "missing" {
            if let Ok(row) = source.material(entry.slot) {
                item.evidence.svr_id = row.svr_id;
                item.evidence.data_index = row.data_index.clone();
                if let Some(media_local_id) = row.local_id.filter(|_| {
                    missing_shards.is_empty()
                        && missing_message_shards.is_empty()
                        && !message_paths.is_empty()
                }) {
                    match crate::adapters::wechat::media::voice::resolve_voice_media_row(
                        &association_sources,
                        &row.chat_username,
                        media_local_id,
                        &row.media_db,
                        row.media_rowid,
                    ) {
                        Ok(proven) if proven.silk == row.voice_data => {
                            item.timestamp = Some(proven.evidence.create_time);
                            item.evidence.timestamp_source = "message";
                            item.message_id = domain::stable_message_id(
                                &row.chat_username,
                                Some(proven.evidence.server_id),
                            );
                            item.sender = proven.sender;
                            item.duration_ms = proven.duration_ms;
                            item.association = "exact_message_media_join".into();
                            item.evidence.message_join = Some(proven.evidence);
                        }
                        Ok(_) => {
                            item.evidence.association_failure =
                                Some("Media Revalidation: StaleEvidence".into())
                        }
                        Err(error) => {
                            item.evidence.association_failure =
                                Some(error.media_error().to_string())
                        }
                    }
                } else {
                    item.evidence.association_failure =
                        Some("Media Discovery: IncompleteSources".into());
                }
                if crate::adapters::wechat::media::voice::is_raw_silk(&row.voice_data) {
                    item.encoding = Some("silk".into());
                    match write_voice_row_for(
                        runtime,
                        &out_dir,
                        &mut names,
                        row,
                        overwrite,
                        Some(&item),
                        task,
                    ) {
                        Ok(voice) => {
                            item.relative_path = Some(voice.relative_path.clone());
                            item.status = "success".into();
                            item.failure = None;
                            exported.push(voice);
                        }
                        Err(error) if task.is_some() => return Err(error),
                        Err(_) => item.failure = Some("Media Publication: Refused".into()),
                    }
                }
            }
        }
        if task.is_some() {
            let mut bytes = serde_json::to_vec(&item)?.len();
            if exported.len() > exported_before {
                bytes = bytes
                    .checked_add(
                        serde_json::to_vec(&task_voice_value(
                            exported.last().context("Missing published voice")?,
                        )?)?
                        .len(),
                    )
                    .ok_or(voice_export::CatalogBudgetExceeded)?;
            }
            summary_budget = summary_budget
                .checked_sub(bytes)
                .ok_or(voice_export::CatalogBudgetExceeded)?;
        }
        manifest.push(item);
    }

    let incomplete_items = source.unmapped_rows
        + missing_shards.len()
        + missing_message_shards.len()
        + manifest
            .iter()
            .filter(|item| item.status != "success" || item.association == "unproven")
            .count();
    let items = if task.is_some() {
        exported
            .iter()
            .map(task_voice_value)
            .collect::<Result<Vec<_>>>()?
    } else {
        exported
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut summary = json!({
        "account_id": runtime.id,
        "output_dir": out_dir.to_string_lossy(),
        "chat_filter": chat,
        "target_username": target_username,
        "scanned_rows": scanned,
        "unmapped_rows": source.unmapped_rows,
        "missing_shards": missing_shards,
        "missing_message_shards": missing_message_shards,
        "partial": incomplete_items > 0,
        "incomplete_items": incomplete_items,
        "associated": manifest.iter().filter(|item| item.association == "exact_message_media_join").count(),
        "exported": exported.len(),
        "items": items,
        "manifest": manifest,
    });
    if task.is_some() {
        summary
            .as_object_mut()
            .context("Invalid voice summary")?
            .remove("output_dir");
    }
    let summary_bytes = serde_json::to_vec_pretty(&summary)?;
    if task.is_some() {
        anyhow::ensure!(
            summary_bytes.len() <= 32 * 1024 * 1024,
            voice_export::CatalogBudgetExceeded
        );
    }
    summary_target.write_bytes_checked(&summary_bytes, || {
        if let Some(task) = task {
            task.verify(runtime)?;
        }
        output_guard.verify()
    })?;
    if let Some(task) = task {
        task.verify(runtime)?;
        crate::daemon::tasks::voice_artifacts::finish(
            runtime,
            task.id,
            task.request,
            incomplete_items.try_into()?,
            false,
            &format!("{:x}", Sha256::digest(&summary_bytes)),
        )?;
    }
    Ok(summary)
}

fn source_path(snapshot: &super::media_snapshot::VoiceSnapshot, source: &str) -> Option<PathBuf> {
    let canonical = source.replace('\\', "/").to_ascii_lowercase();
    snapshot
        .snapshot
        .sources()
        .iter()
        .find(|s| s.source.replace('\\', "/").to_ascii_lowercase() == canonical)
        .map(|s| s.path.clone())
}

fn task_voice_value(voice: &ExportedVoice) -> Result<Value> {
    let mut value = serde_json::to_value(voice)?;
    let object = value.as_object_mut().context("Invalid voice evidence")?;
    object.remove("audio_file");
    object.remove("evidence_file");
    object.insert("timestamp_source".into(), json!("media"));
    Ok(value)
}

fn task_voice_evidence(
    voice: &ExportedVoice,
    association: Option<&domain::ManifestItem<voice_export::ExportEvidence>>,
) -> Result<Value> {
    let mut evidence = task_voice_value(voice)?;
    let message = association.and_then(|item| item.evidence.message_join.as_ref());
    evidence["message_timestamp"] = json!(message.map(|join| join.create_time));
    evidence["message_timestamp_source"] = json!(message.map(|_| "message"));
    evidence["evidence"] = serde_json::to_value(association.map(|item| &item.evidence))?;
    Ok(evidence)
}

#[cfg(test)]
fn write_voice_row(
    runtime: &RuntimeContext,
    out_root: &Path,
    names: &mut Names,
    row: VoiceRow,
    overwrite: bool,
    association: Option<&domain::ManifestItem<voice_export::ExportEvidence>>,
) -> Result<ExportedVoice> {
    write_voice_row_for(runtime, out_root, names, row, overwrite, association, None)
}

fn write_voice_row_for(
    runtime: &RuntimeContext,
    out_root: &Path,
    names: &mut Names,
    row: VoiceRow,
    overwrite: bool,
    association: Option<&domain::ManifestItem<voice_export::ExportEvidence>>,
    task: Option<&TaskPublication<'_>>,
) -> Result<ExportedVoice> {
    if let Some(task) = task {
        task.verify(runtime)?;
    }
    let chat_type = chat_type_of(&row.chat_username, names).to_string();
    let lane = if chat_type == "group" {
        "群聊"
    } else {
        "一对一聊天"
    };
    let display = names.display(&row.chat_username);
    let safe_display = sanitize_path_component(&display);
    let chat_dir = out_root
        .join(format!("account-{:x}", md5::compute(runtime.id.as_bytes())))
        .join(lane)
        .join(format!(
            "{}--{:x}",
            safe_display,
            md5::compute(row.chat_username.as_bytes())
        ));
    crate::infrastructure::publication::validate_export_target(runtime, &chat_dir)?;
    std::fs::create_dir_all(&chat_dir)?;

    let stem = format!(
        "{}_{:x}_{}",
        row.create_time
            .map(|time| time.to_string())
            .unwrap_or_else(|| "unknown".into()),
        md5::compute(row.media_db.as_bytes()),
        row.media_rowid
    );
    let silk_path = chat_dir.join(format!("{stem}.silk"));
    let json_path = chat_dir.join(format!("{stem}.voice.json"));
    if !overwrite && (silk_path.exists() || json_path.exists()) {
        bail!(
            "目标已存在: {}（需要覆盖请加 --overwrite）",
            silk_path.display()
        );
    }

    let audio_target = capture_voice_target(runtime, &silk_path, overwrite)?;
    let evidence_target = capture_voice_target(runtime, &json_path, overwrite)?;

    let silk_header_ok = crate::adapters::wechat::media::voice::is_raw_silk(&row.voice_data);
    anyhow::ensure!(silk_header_ok, "invalid raw SILK container");
    let raw_had_0x02_prefix = row.voice_data.first() == Some(&2);
    let audio_sha256 = format!("{:x}", Sha256::digest(&row.voice_data));
    audio_target.write_bytes_checked(&row.voice_data, || {
        if let Some(task) = task {
            task.pin.verify(runtime)?;
        }
        Ok(())
    })?;
    #[cfg(test)]
    if task.is_some_and(|task| matches!(task.writer_fault, Some(WriterFault::SecondPublication))) {
        std::fs::write(&json_path, b"synthetic concurrent sidecar")?;
    }

    let time = row.create_time.map(fmt_time);
    let exported = ExportedVoice {
        chat: display,
        chat_username: row.chat_username,
        chat_type,
        timestamp: row.create_time,
        time,
        local_id: row.local_id,
        svr_id: row.svr_id,
        chat_name_id: row.chat_name_id,
        data_index: row.data_index,
        media_rowid: row.media_rowid,
        relative_path: silk_path
            .strip_prefix(out_root)?
            .to_string_lossy()
            .replace('\\', "/"),
        message_id: association.and_then(|item| item.message_id.clone()),
        sender: association.and_then(|item| item.sender.clone()),
        duration_ms: association.and_then(|item| item.duration_ms),
        association: association
            .map(|item| item.association.clone())
            .unwrap_or_else(|| "unproven".into()),
        media_db: row.media_db,
        audio_file: silk_path.to_string_lossy().into_owned(),
        evidence_file: json_path.to_string_lossy().into_owned(),
        audio_format: "silk".to_string(),
        voice_data_bytes: row.voice_data.len(),
        raw_had_0x02_prefix,
        silk_header_ok,
    };
    let bytes = if task.is_some() {
        serde_json::to_vec_pretty(&task_voice_evidence(&exported, association)?)?
    } else {
        serde_json::to_vec_pretty(&exported)?
    };
    if task.is_some() {
        anyhow::ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            voice_export::CatalogBudgetExceeded
        );
    }
    evidence_target.write_bytes_checked(&bytes, || {
        if let Some(task) = task {
            task.pin.verify(runtime)?;
        }
        Ok(())
    })?;
    #[cfg(test)]
    if task.is_some_and(|task| matches!(task.writer_fault, Some(WriterFault::SecondRegistration))) {
        std::fs::write(&json_path, b"synthetic changed sidecar")?;
    }
    if let Some(task) = task {
        task.verify(runtime)?;
        crate::daemon::tasks::voice_artifacts::published_group(
            runtime,
            task.id,
            task.request,
            [
                (&silk_path, &audio_sha256),
                (&json_path, &format!("{:x}", Sha256::digest(&bytes))),
            ],
            exported.association == "exact_message_media_join",
        )?;
    }
    Ok(exported)
}

fn capture_voice_target(
    runtime: &RuntimeContext,
    path: &Path,
    overwrite: bool,
) -> Result<ExportTarget> {
    if overwrite {
        ExportTarget::capture(runtime, path)
    } else {
        ExportTarget::new_file(
            path,
            &crate::infrastructure::publication::export_protected(runtime),
        )
    }
}

fn sanitize_path_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars().take(64) {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed
    }
}

fn fmt_time(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::fs;

    fn runtime(root: &Path) -> RuntimeContext {
        RuntimeContext {
            config: crate::config::Config {
                key_store: Some(root.join("store.dpapi")),
                db_dir: root.join("db"),
                keys_file: root.join("keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.into(),
            id: "synthetic".into(),
            directory: root.join("runtime"),
        }
    }
    fn names() -> Names {
        Names {
            map: HashMap::from([("chat".into(), "name".into())]),
            msg_db_keys: Vec::new(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        }
    }
    fn row() -> VoiceRow {
        VoiceRow {
            media_rowid: 1,
            chat_name_id: 1,
            chat_username: "chat".into(),
            create_time: Some(10),
            local_id: Some(2),
            svr_id: Some(3),
            data_index: None,
            voice_data: b"\x02#!SILK_V3synthetic".to_vec(),
            media_db: "message/media_0.db".into(),
        }
    }

    fn actual_task_writer_failure(fault: WriterFault, number: u64) {
        let base = tempfile::tempdir().unwrap();
        let (runtime, mut task) =
            crate::daemon::tasks::voice_artifacts::writer_test_fixture(base.path(), number);
        // Store binds to the canonical database directory, unlike the index-only fixture.
        fs::create_dir_all(&runtime.config.db_dir).unwrap();
        let keys = crate::service::worker_keys::DatabaseKeys(HashMap::from([(
            "message/media_0.db".into(),
            "11".repeat(32),
        )]));
        crate::key_store::Store::for_runtime(&runtime)
            .unwrap()
            .update(
                None,
                &[crate::key_store::Update::Databases(
                    &keys.0,
                    crate::key_store::Verification::Verified,
                )],
            )
            .unwrap();
        let request = task.options.voice_export.clone().unwrap();
        let mut publication = TaskPublication {
            id: &task.id,
            request: &request,
            pin: crate::service::config_pin::ConfigPin::new(&runtime).unwrap(),
            material: material_digest(&keys.0),
            synthetic_store: true,
            writer_fault: None,
        };
        let output = task.output_dir.join("voices");
        let first = write_voice_row_for(
            &runtime,
            &output,
            &mut names(),
            row(),
            false,
            None,
            Some(&publication),
        )
        .unwrap();
        let evidence = fs::read(&first.evidence_file).unwrap();
        let first_name = Path::new(&first.audio_file)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let first_evidence_name = Path::new(&first.evidence_file)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        publication.writer_fault = Some(fault);
        let mut second = row();
        second.media_rowid = 2;
        let failure = write_voice_row_for(
            &runtime,
            &output,
            &mut names(),
            second,
            false,
            None,
            Some(&publication),
        )
        .unwrap_err();
        if matches!(fault, WriterFault::SecondRegistration) {
            assert_eq!(
                failure
                    .downcast_ref::<crate::service::protocol::ServiceError>()
                    .unwrap()
                    .code,
                "artifact_changed"
            );
        }
        drop(publication);
        let artifacts =
            crate::daemon::tasks::voice_artifacts::writer_test_terminal(&runtime, &mut task);
        assert_eq!(artifacts.len(), 2);
        for (name, bytes) in artifacts {
            if name == first_name {
                assert_eq!(bytes, row().voice_data);
            } else {
                assert_eq!(name, first_evidence_name);
                assert_eq!(bytes, evidence);
            }
        }
    }

    #[test]
    fn task_writer_second_sidecar_publication_failure_keeps_readable_prefix() {
        actual_task_writer_failure(WriterFault::SecondPublication, 31);
    }

    #[test]
    fn task_writer_second_registration_failure_keeps_readable_prefix() {
        actual_task_writer_failure(WriterFault::SecondRegistration, 32);
    }

    #[test]
    fn fixed_runtime_raw_audio_evidence_and_no_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::write(&runtime.config_path, b"changed invalid configuration").unwrap();
        let output = root.path().join("out");
        let first = write_voice_row(&runtime, &output, &mut names(), row(), false, None).unwrap();
        assert_eq!(fs::read(&first.audio_file).unwrap(), row().voice_data);
        let evidence: Value =
            serde_json::from_slice(&fs::read(&first.evidence_file).unwrap()).unwrap();
        assert_eq!(evidence["raw_had_0x02_prefix"], true);
        assert_eq!(evidence["voice_data_bytes"], row().voice_data.len());
        assert!(write_voice_row(&runtime, &output, &mut names(), row(), false, None).is_err());
        write_voice_row(&runtime, &output, &mut names(), row(), true, None).unwrap();
        assert_eq!(
            fs::read(&runtime.config_path).unwrap(),
            b"changed invalid configuration"
        );
    }

    #[test]
    fn all_artifacts_reject_account_paths_and_hardlink_aliases() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        for protected in [
            &runtime.config_path,
            &runtime.config.keys_file,
            runtime.config.key_store.as_ref().unwrap(),
        ] {
            fs::write(protected, b"protected synthetic bytes").unwrap();
            assert!(capture_voice_target(&runtime, protected, true).is_err());
        }
        for protected in [
            &runtime.config.db_dir,
            &runtime.config.decrypted_dir,
            &runtime.directory,
        ] {
            assert!(write_voice_row(&runtime, protected, &mut names(), row(), true, None).is_err());
            assert!(!protected.exists());
        }
        let output = root.path().join("out");
        fs::create_dir(&output).unwrap();
        for name in ["10_2.silk", "10_2.voice.json", "_voice_export_summary.json"] {
            let path = output.join(name);
            fs::hard_link(&runtime.config.keys_file, &path).unwrap();
            assert!(capture_voice_target(&runtime, &path, true).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"protected synthetic bytes");
        }
    }

    #[test]
    fn raw_paths_isolate_accounts_chats_and_media_rows_without_repair() {
        let root = tempfile::tempdir().unwrap();
        let mut runtime = runtime(root.path());
        let output = root.path().join("out");
        let mut names = names();
        names.map.insert("other".into(), "name".into());
        let first = write_voice_row(&runtime, &output, &mut names, row(), false, None).unwrap();
        let mut other = row();
        other.chat_username = "other".into();
        other.voice_data = b"#!SILK_V3\0\x7f".to_vec();
        let second = write_voice_row(&runtime, &output, &mut names, other, false, None).unwrap();
        assert_ne!(first.relative_path, second.relative_path);
        assert_eq!(fs::read(&second.audio_file).unwrap(), b"#!SILK_V3\0\x7f");
        let mut other = row();
        other.media_db = "message/media_1.db".into();
        let third = write_voice_row(&runtime, &output, &mut names, other, false, None).unwrap();
        assert_ne!(first.relative_path, third.relative_path);
        runtime.id = "other-account".into();
        let fourth = write_voice_row(&runtime, &output, &mut names, row(), false, None).unwrap();
        assert_ne!(first.relative_path, fourth.relative_path);
    }

    #[test]
    fn evidence_publication_failure_keeps_audio_and_old_evidence() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let output = root.path().join("out");
        let first = write_voice_row(&runtime, &output, &mut names(), row(), false, None).unwrap();
        fs::write(&first.audio_file, b"old audio").unwrap();
        fs::write(&first.evidence_file, b"old evidence").unwrap();
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&first.evidence_file)
            .unwrap();
        assert!(write_voice_row(&runtime, &output, &mut names(), row(), true, None).is_err());
        assert_eq!(fs::read(&first.audio_file).unwrap(), row().voice_data);
        assert_eq!(fs::read(&first.evidence_file).unwrap(), b"old evidence");
        assert_eq!(
            fs::read_dir(Path::new(&first.audio_file).parent().unwrap())
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn task_projection_removes_host_paths_and_preserves_media_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let output = root.path().join("out");
        let voice = write_voice_row(&runtime, &output, &mut names(), row(), false, None).unwrap();
        let projection = task_voice_value(&voice).unwrap();
        assert!(projection.get("audio_file").is_none());
        assert!(projection.get("evidence_file").is_none());
        assert_eq!(projection["timestamp"], 10);
        assert_eq!(projection["timestamp_source"], "media");
        assert_eq!(projection["media_db"], "message/media_0.db");
        assert!(!serde_json::to_string(&projection)
            .unwrap()
            .contains(&root.path().to_string_lossy().replace('\\', "\\\\")));
    }

    #[test]
    fn typed_projection_keeps_media_and_message_times_distinct_without_relaxing_resolver() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let output = root.path().join("out");
        // This is a serializer input, not a claim that the strict resolver accepts unequal times.
        let association = domain::ManifestItem {
            account_id: runtime.id.clone(),
            message_id: Some("synthetic-message".into()),
            conversation: Some("chat".into()),
            sender: None,
            timestamp: Some(20),
            duration_ms: None,
            encoding: Some("silk".into()),
            relative_path: None,
            status: "success".into(),
            association: "exact_message_media_join".into(),
            failure: None,
            evidence: voice_export::ExportEvidence {
                media_source: "message/media_0.db".into(),
                media_rowid: 1,
                media_chat_name_id: 1,
                media_local_id: Some(2),
                media_create_time: Some(10),
                timestamp_source: "message",
                svr_id: Some(3),
                data_index: None,
                association_failure: None,
                message_join: Some(crate::adapters::wechat::media::voice::VoiceEvidence {
                    username: "chat".into(),
                    message_source: "message/message_0.db".into(),
                    message_table: "Msg_synthetic".into(),
                    message_local_id: 7,
                    server_id: 3,
                    create_time: 20,
                    media_source: "message/media_0.db".into(),
                    media_rowid: 1,
                    media_chat_name_id: 1,
                    media_local_id: 2,
                }),
            },
        };
        let voice = write_voice_row(
            &runtime,
            &output,
            &mut names(),
            row(),
            false,
            Some(&association),
        )
        .unwrap();
        let item = task_voice_value(&voice).unwrap();
        let sidecar = task_voice_evidence(&voice, Some(&association)).unwrap();
        let manifest = serde_json::to_value(&association).unwrap();
        assert_eq!(item["timestamp"], 10);
        assert_eq!(item["timestamp_source"], "media");
        assert_eq!(sidecar["timestamp"], 10);
        assert_eq!(sidecar["timestamp_source"], "media");
        assert_eq!(sidecar["message_timestamp"], 20);
        assert_eq!(sidecar["message_timestamp_source"], "message");
        assert_eq!(manifest["timestamp"], 20);
        assert_eq!(manifest["evidence"]["timestamp_source"], "message");
        assert_eq!(manifest["evidence"]["media_create_time"], 10);
        assert!(Path::new(&voice.audio_file)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("10_"));
        assert!(sidecar.get("audio_file").is_none() && sidecar.get("evidence_file").is_none());
        let unknown = task_voice_evidence(&voice, None).unwrap();
        assert!(
            unknown["message_timestamp"].is_null() && unknown["message_timestamp_source"].is_null()
        );
    }

    #[test]
    fn captured_summary_refuses_concurrent_replacement() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let path = root.path().join("_voice_export_summary.json");
        fs::write(&path, b"old summary").unwrap();
        let summary = ExportTarget::capture(&runtime, &path).unwrap();
        fs::write(&path, b"concurrent summary").unwrap();
        assert!(summary.write_bytes(b"new summary").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"concurrent summary");
    }
}
