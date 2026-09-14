//! 旧批量导出的统一原生宿主；模式选择、账号固定和转录在同一流程内完成。
use crate::{
    ipc::Request,
    message::export::Target,
    runtime::RuntimeContext,
    toolkit::{chat_delta::DeltaWindow, chat_plan::PlanChat, chat_plan_selection::Plan},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub use crate::service::operation_requests::export_all::Args;

pub(super) fn emit(report: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&report)?);
    report_outcome(&report).require_success()?;
    Ok(())
}

fn report_outcome(report: &Value) -> crate::ipc::outcome::BusinessOutcome {
    use crate::ipc::outcome::BusinessOutcome;
    let mut succeeded = report["written"].as_u64().unwrap_or(0);
    let mut failed = report["failures"]
        .as_array()
        .map_or(0, |items| items.len() as u64);
    if let Some(results) = report["results"].as_array() {
        for item in results {
            if BusinessOutcome::from_legacy(item) == BusinessOutcome::Success {
                succeeded = succeeded.saturating_add(1);
            } else {
                failed = failed.saturating_add(1);
            }
        }
    }
    for item in report["transcriptions"].as_array().into_iter().flatten() {
        succeeded = succeeded.saturating_add(item["transcribed"].as_u64().unwrap_or(0));
        failed = failed.saturating_add(item["failed"].as_u64().unwrap_or(0));
        if item["warnings"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            failed = failed.saturating_add(1);
        }
    }
    if failed != 0 {
        BusinessOutcome::from_counts(succeeded, failed)
    } else {
        BusinessOutcome::from_legacy(report)
    }
}

#[test]
fn aggregate_outcome_preserves_partial_exports_and_asr_failures() {
    use crate::ipc::outcome::BusinessOutcome;
    for (report, expected) in [
        (json!({"written":2,"failures":[]}), BusinessOutcome::Success),
        (
            json!({"written":1,"failures":[{"error":"PRIVATE"}]}),
            BusinessOutcome::Partial,
        ),
        (
            json!({"written":0,"failures":[{"error":"PRIVATE"}]}),
            BusinessOutcome::Failure,
        ),
        (
            json!({"success":false,"results":[{"success":true},{"success":false}]}),
            BusinessOutcome::Partial,
        ),
        (
            json!({"written":1,"transcriptions":[{"failed":1}]}),
            BusinessOutcome::Partial,
        ),
        (
            json!({"written":1,"transcriptions":[{"failed":0,"warnings":["PRIVATE"]}]}),
            BusinessOutcome::Partial,
        ),
    ] {
        assert_eq!(report_outcome(&report), expected);
    }
}

pub(super) fn export_for(runtime: &RuntimeContext, args: Args) -> Result<Value> {
    args.validate()?;
    let output = std::path::absolute(
        args.output_dir
            .clone()
            .unwrap_or_else(|| runtime.config_path.parent().unwrap().join("exported_chats")),
    )?;
    super::export_chat::validate_output_for(runtime, &output)?;
    if args.write_plan_csv.is_some() {
        return write_plan(runtime, &args);
    }
    if args.delta_only {
        return export_delta(runtime, args, &output);
    }
    let has_plan = args.from_plan_csv.is_some();
    let native = super::export_chats::Args {
        output_dir: output,
        users: args.users,
        incremental: args.incremental,
        start: args.start,
        end: args.end,
        dry_run: args.dry_run,
        from_plan_csv: args.from_plan_csv,
        plan_mode: Some(args.plan_mode).filter(|_| has_plan),
    };
    if !args.with_transcriptions || native.dry_run {
        return super::export_chats::export_for(runtime, native, None);
    }
    let mut transcriber = super::asr_batch::prepare(runtime, args.asr)?;
    let mut reports = Vec::new();
    let mut process = |_: &Target, document: &mut Value| -> Result<()> {
        reports.push(serde_json::to_value(transcriber.process(document)?)?);
        Ok(())
    };
    let mut report = super::export_chats::export_for(runtime, native, Some(&mut process))?;
    report["transcriptions"] = reports.into();
    Ok(report)
}

fn selected(runtime: &RuntimeContext, args: &Args) -> Result<Vec<Target>> {
    let response = crate::service::query_client::send_for(runtime, Request::ExportChatList)?;
    let mut targets: Vec<Target> = serde_json::from_value(response.data["chats"].clone())?;
    let filter = args
        .users
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| std::env::var("WECHAT_EXPORT_USERS").unwrap_or_default());
    if !filter.trim().is_empty() {
        let wanted: HashSet<_> = filter
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        targets.retain(|target| wanted.contains(target.username.as_str()));
        ensure!(!targets.is_empty(), "指定 username 列表跟会话表没有交集");
    }
    if let Some(path) = &args.from_plan_csv {
        let plan = Plan::load(path, args.plan_mode)?;
        let order = plan.select(targets.iter().map(|t| t.username.as_str()))?;
        let mut by_name: std::collections::HashMap<_, _> = targets
            .into_iter()
            .map(|target| (target.username.clone(), target))
            .collect();
        targets = order
            .iter()
            .map(|name| by_name.remove(name).expect("计划身份已校验"))
            .collect();
    }
    Ok(targets)
}

fn shards(root: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let directory = root.join("message");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(".db"))
        else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        ensure!(entry.file_type()?.is_file(), "数据库分片不是普通文件");
        paths.push(PathBuf::from("message").join(name));
    }
    paths.sort();
    Ok(paths)
}

fn write_plan(runtime: &RuntimeContext, args: &Args) -> Result<Value> {
    let output = std::path::absolute(args.write_plan_csv.as_ref().unwrap())?;
    super::export_chat::validate_output_for(runtime, &output)?;
    let targets = selected(runtime, args)?;
    let chats = targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| PlanChat {
            index: index + 1,
            username: target.username,
            chat_name: target.chat,
            chat_type: if target.is_group { "group" } else { "single" }.into(),
        })
        .collect();
    let root = &runtime.config.decrypted_dir;
    let resource = PathBuf::from("message/message_resource.db");
    let resource = if root.join(&resource).try_exists()? {
        Some(resource)
    } else {
        let mut paths = shards(root, "message_resource_")?;
        ensure!(
            paths.len() <= 1,
            "当前计划统计需要单一 message_resource 库，不能静默忽略其他分片"
        );
        paths.pop()
    };
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
            .context("账号目录缺少父目录")?
            .to_owned()
    } else {
        runtime.config.db_dir.clone()
    };
    let native = super::chat_plan::Args {
        decrypted_dir: root.clone(),
        message_dbs: shards(root, "message_")?,
        resource_db: resource,
        media_dbs: shards(root, "media_")?,
        users: Vec::new(),
        chats_json: None,
        exclude_users: Vec::new(),
        size_mode: args.size_mode,
        source_dir: (args.size_mode == super::chat_plan::Mode::Scan).then_some(source),
        media_dir: None,
        threads: 1,
        start: args.start.clone(),
        end: args.end.clone(),
        output: output.clone(),
    };
    let count = super::chat_plan::execute_for(&native, Some(chats), Some(""), true)?;
    Ok(json!({"engine":"rust", "mode":"plan", "path":output, "chats":count}))
}

fn export_delta(runtime: &RuntimeContext, args: Args, output: &Path) -> Result<Value> {
    let users: Vec<_> = selected(runtime, &args)?
        .into_iter()
        .map(|target| target.username)
        .collect();
    if args.dry_run || users.is_empty() {
        return Ok(
            json!({"engine":"rust", "mode":"delta", "planned":users.len(), "users":users, "dry_run":args.dry_run, "success":true}),
        );
    }
    let now = chrono::Local::now();
    let window = DeltaWindow {
        start: args
            .start
            .as_deref()
            .map(super::export_chats::parse_timestamp)
            .transpose()?,
        end: args
            .end
            .as_deref()
            .map(super::export_chats::parse_timestamp)
            .transpose()?,
        run_id: now.format("%Y%m%d_%H%M%S_%9f").to_string(),
        utc_offset_seconds: now.offset().local_minus_utc(),
        generated_at: now.format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    let mut transcriber = if args.with_transcriptions {
        Some(super::asr_batch::prepare(runtime, args.asr)?)
    } else {
        None
    };
    let mut reports = Vec::new();
    let mut report = super::export_delta::export_delta_with_mode(
        output,
        &users,
        window,
        output.try_exists()?,
        &crate::toolkit::export_protected(runtime),
        |query| {
            let mut value = crate::service::query_client::send_for(
                runtime,
                Request::ExportDelta {
                    username: query.username,
                    start: query.start,
                    end: query.end,
                },
            )?
            .data;
            if let Some(transcriber) = &mut transcriber {
                reports.push(serde_json::to_value(
                    transcriber.process_delta(&mut value)?,
                )?);
            }
            Ok(value)
        },
    )?;
    report["engine"] = "rust".into();
    report["transcriptions"] = reports.into();
    Ok(report)
}
