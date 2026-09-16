//! 原生批量导出编排；计划 CSV 只选择 username，不串联语音转录。
use crate::application::{chat_archive_index::ChatIndex, chat_plan_selection::Plan};
use crate::business::archive as domain;
use crate::runtime::RuntimeContext;
#[cfg(test)]
use crate::service::operation_requests::plan::Mode;
use crate::{ipc::Request, message::export::Target};
use anyhow::{ensure, Context, Result};
use chrono::{Local, TimeZone};
use serde::Serialize;

pub use crate::service::operation_requests::export_chats::Args;

#[derive(Default)]
struct TimeRange {
    start: Option<i64>,
    end: Option<i64>,
}

pub use crate::service::time::parse_timestamp;

impl TimeRange {
    fn parse(start: Option<&str>, end: Option<&str>) -> Result<Self> {
        let range = Self {
            start: start.map(parse_timestamp).transpose()?,
            end: end.map(parse_timestamp).transpose()?,
        };
        if let (Some(start), Some(end)) = (range.start, range.end) {
            ensure!(start <= end, "起始时间不能晚于结束时间");
        }
        Ok(range)
    }

    fn apply(&self, document: &mut serde_json::Value) -> Result<usize> {
        let messages = document["messages"]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("导出缺少消息数组"))?;
        if self.start.is_some() || self.end.is_some() {
            // 与 SQLite 时间条件一致：NULL 不参与有界范围比较。
            messages.retain(|message| {
                message["timestamp"].as_i64().is_some_and(|time| {
                    self.start.is_none_or(|start| time >= start)
                        && self.end.is_none_or(|end| time <= end)
                })
            });
        }
        let date = |message: Option<&serde_json::Value>| {
            message
                .and_then(|m| m["timestamp"].as_i64())
                .and_then(|time| Local.timestamp_opt(time, 0).single())
                .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default()
        };
        let first = date(messages.first());
        let last = date(messages.last());
        let count = messages.len();
        document["date_first_msg"] = first.into();
        document["date_last_msg"] = last.into();
        Ok(count)
    }
}

#[derive(Serialize)]
struct Failure {
    username: String,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_published: Option<bool>,
}

pub fn cmd_export(args: Args) -> Result<()> {
    let has_plan = args.from_plan_csv.is_some();
    let summary = export_with(None, args, None, crate::service::query_client::send_for)?;
    // 保留空计划的紧凑输出，其他结果仍采用原有缩进格式。
    if has_plan && summary.get("total") == Some(&serde_json::json!(0)) {
        println!("{summary}");
    } else {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }
    let failures = summary["failures"].as_array().map_or(0, Vec::len);
    crate::ipc::outcome::BusinessOutcome::from_counts(
        summary["written"].as_u64().unwrap_or(0),
        failures as u64,
    )
    .require_success()?;
    Ok(())
}

/// 处理最终待保存文档；可补充转录等字段，不应改变聊天身份或消息集合。
pub type DocumentProcessor<'a> = dyn FnMut(&Target, &mut serde_json::Value) -> Result<()> + 'a;

/// 使用调用方固定的账号，返回原有结果 JSON；单聊失败位于 failures，批次继续。
/// None 不修改文档；回调在合并和日期重算后运行，失败时不发布该聊天。
pub fn export_for(
    runtime: &RuntimeContext,
    args: Args,
    processor: Option<&mut DocumentProcessor<'_>>,
) -> Result<serde_json::Value> {
    export_with(
        Some(runtime),
        args,
        processor,
        crate::service::query_client::send_for,
    )
}

// 仅注入请求边界，测试仍执行真实选择、合并、回调与原子文件发布。
struct BatchArchive<'a, 'p, 'd, F> {
    runtime: &'a RuntimeContext,
    targets: &'a [Target],
    index: &'a mut ChatIndex,
    range: &'a TimeRange,
    incremental: bool,
    processor: Option<&'p mut DocumentProcessor<'d>>,
    send: F,
    next: usize,
    path: Option<std::path::PathBuf>,
    destination: Option<crate::infrastructure::publication::ExportTarget>,
}

fn archive_failure(stage: domain::Stage, error: anyhow::Error) -> domain::Failure {
    domain::Failure {
        stage,
        detail: error.to_string(),
    }
}

pub(super) fn filter_targets(targets: &mut Vec<Target>, raw: &str) -> Result<()> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    let requested: Vec<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let selected = domain::select_usernames(
        targets.iter().map(|target| target.username.as_str()),
        &requested,
    )
    .context("指定 username 列表跟会话表没有交集")?;
    targets.retain(|target| selected.contains(&target.username));
    Ok(())
}

impl<F> domain::FullArchive for BatchArchive<'_, '_, '_, F>
where
    F: FnMut(&RuntimeContext, Request) -> Result<crate::ipc::Response>,
{
    type Document = serde_json::Value;

    fn prepare(&mut self, username: &str) -> Result<(), domain::Failure> {
        (|| -> Result<()> {
            self.path = None;
            self.destination = None;
            let target = self
                .targets
                .get(self.next)
                .context("archive target unavailable")?;
            self.next += 1;
            ensure!(target.username == username, "archive target order changed");
            eprintln!(
                "[{}/{}] 导出 {}",
                self.next,
                self.targets.len(),
                target.chat
            );
            let path = self.index.choose(username, &target.chat, target.is_group)?;
            super::export_chat::validate_output_for(self.runtime, &path)?;
            self.destination = Some(crate::infrastructure::publication::ExportTarget::capture(
                self.runtime,
                &path,
            )?);
            self.path = Some(path);
            Ok(())
        })()
        .map_err(|error| archive_failure(domain::Stage::Prepare, error))
    }

    fn read(
        &mut self,
        username: &str,
    ) -> Result<domain::RawArchive<Self::Document>, domain::Failure> {
        let response = (self.send)(
            self.runtime,
            Request::ExportChatByUsername {
                username: username.to_owned(),
            },
        )
        .map_err(|error| archive_failure(domain::Stage::Read, error))?;
        Ok(domain::RawArchive {
            username: response.data["username"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            document: response.data,
        })
    }

    fn transform(
        &mut self,
        username: &str,
        mut document: Self::Document,
    ) -> Result<domain::PreparedArchive<Self::Document>, domain::Failure> {
        (|| -> Result<_> {
            let target = self
                .targets
                .get(self.next - 1)
                .context("archive target unavailable")?;
            let path = self.path.as_ref().context("archive output not prepared")?;
            let mut added = self.range.apply(&mut document)?;
            if self.incremental {
                let previous = self
                    .index
                    .current(username)?
                    .or_else(|| path.exists().then(|| path.clone()));
                if let Some(previous) = previous {
                    let old = serde_json::from_reader(std::io::BufReader::new(
                        std::fs::File::open(previous)?,
                    ))?;
                    let merged =
                        crate::application::chat_archive_merge::merge_chat_json(&old, &document)?;
                    added = merged.report.added;
                    let mut merged_document = merged.document;
                    for key in ["chat", "exported_at"] {
                        merged_document[key] = document[key].clone();
                    }
                    for key in [
                        "contact_remark",
                        "contact_nick_name",
                        "contact_memo",
                        "contact_tags",
                        "metadata_warnings",
                    ] {
                        if let Some(value) = document.get(key) {
                            merged_document[key] = value.clone();
                        } else {
                            merged_document.as_object_mut().unwrap().remove(key);
                        }
                    }
                    if target.is_group {
                        merged_document["is_group"] = true.into();
                    } else {
                        merged_document.as_object_mut().unwrap().remove("is_group");
                    }
                    document = merged_document;
                }
            }
            // The requested range constrains additions, not messages already archived.
            let messages = TimeRange::default().apply(&mut document)?;
            if let Some(processor) = self.processor.as_deref_mut() {
                processor(target, &mut document)?;
            }
            Ok(domain::PreparedArchive {
                archive: domain::RawArchive {
                    username: document["username"].as_str().unwrap_or_default().to_owned(),
                    document,
                },
                messages,
                added_messages: added,
            })
        })()
        .map_err(|error| archive_failure(domain::Stage::Transform, error))
    }

    fn publish(&mut self, document: &Self::Document) -> Result<(), domain::Failure> {
        (|| -> Result<()> {
            self.destination
                .take()
                .context("archive output not prepared")?
                .write_json(document)
        })()
        .map_err(|error| archive_failure(domain::Stage::Publish, error))
    }

    fn record(&mut self, username: &str) -> Result<(), domain::Failure> {
        (|| -> Result<()> {
            self.index.record(
                self.path.as_ref().context("archive output not prepared")?,
                username,
            )
        })()
        .map_err(|error| archive_failure(domain::Stage::Index, error))
    }
}

// Test injection uses the real domain workflow, raw conversion and guarded publication.
fn export_with(
    runtime: Option<&RuntimeContext>,
    args: Args,
    processor: Option<&mut DocumentProcessor<'_>>,
    mut send: impl FnMut(&RuntimeContext, Request) -> Result<crate::ipc::Response>,
) -> Result<serde_json::Value> {
    let Args {
        output_dir: output,
        users,
        dry_run,
        start,
        end,
        incremental,
        from_plan_csv,
        plan_mode,
    } = args;
    ensure!(
        from_plan_csv.is_some() || plan_mode.is_none(),
        "--plan-mode 需要 --from-plan-csv"
    );
    let range = TimeRange::parse(start.as_deref(), end.as_deref())?;
    let plan = from_plan_csv
        .as_deref()
        .map(|path| Plan::load(path, plan_mode.unwrap_or_default()))
        .transpose()?;
    // CLI 先校验时间和计划，再加载一次账号；程序化入口始终沿用传入的上下文。
    let loaded;
    let runtime = match runtime {
        Some(runtime) => runtime,
        None => {
            loaded = RuntimeContext::load()?;
            &loaded
        }
    };
    let output = std::path::absolute(output)?;
    super::export_chat::validate_output_for(runtime, &output)?;
    // 索引和锁文件也必须避开用户自行配置的密钥及配置文件名。
    for name in ["_export_index.json", ".wx-export.lock"] {
        super::export_chat::validate_output_for(runtime, &output.join(name))?;
    }
    let response = send(runtime, Request::ExportChatList)?;
    let mut targets: Vec<Target> = serde_json::from_value(response.data["chats"].clone())?;
    let raw = users
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("WECHAT_EXPORT_USERS").unwrap_or_default());
    filter_targets(&mut targets, &raw)?;
    if let Some(plan) = &plan {
        let selected = plan.select(targets.iter().map(|target| target.username.as_str()))?;
        let mut by_username: std::collections::HashMap<_, _> = targets
            .into_iter()
            .map(|target| (target.username.clone(), target))
            .collect();
        targets = selected
            .iter()
            .map(|username| {
                by_username
                    .remove(username)
                    .expect("validated plan username")
            })
            .collect();
    }
    if dry_run {
        return Ok(
            serde_json::json!({"engine":"rust","planned":targets.len(),"chats":targets,"start_ts":range.start,"end_ts":range.end,"incremental":incremental}),
        );
    }
    if plan.is_some() && targets.is_empty() {
        return Ok(
            serde_json::json!({"engine":"rust","total":0,"written":0,"messages":0,"added_messages":0,"incremental":incremental,"failures":[]}),
        );
    }
    let mut index = ChatIndex::open_for_runtime(&output, runtime)?;
    let legacy_unverified = index.legacy_unverified();
    let usernames: Vec<_> = targets
        .iter()
        .map(|target| target.username.clone())
        .collect();
    let results = domain::export_chats(
        &usernames,
        &mut BatchArchive {
            runtime,
            targets: &targets,
            index: &mut index,
            range: &range,
            incremental,
            processor,
            send,
            next: 0,
            path: None,
            destination: None,
        },
    );
    let mut written = 0;
    let mut messages = 0;
    let mut added_messages = 0;
    let mut failures = Vec::new();
    for target in results {
        match target.result {
            Ok((count, added)) => {
                written += 1;
                messages += count;
                added_messages += added;
            }
            Err(error) => failures.push(Failure {
                username: target.username,
                error: match error.stage {
                    domain::Stage::SourceIdentity => "返回的聊天身份与请求不符".into(),
                    domain::Stage::ProcessedIdentity => "处理后的聊天身份与请求不符".into(),
                    _ => error.detail,
                },
                artifact_published: target.artifact_published.then_some(true),
            }),
        }
    }
    let mut report = serde_json::json!({"engine":"rust","total":targets.len(),"written":written,"messages":messages,"added_messages":added_messages,"incremental":incremental,"failures":failures});
    if legacy_unverified {
        report["legacy_unverified"] = true.into();
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn export_with(
        runtime: &RuntimeContext,
        args: Args,
        processor: Option<&mut DocumentProcessor<'_>>,
        send: impl FnMut(&RuntimeContext, Request) -> Result<crate::ipc::Response>,
    ) -> Result<serde_json::Value> {
        super::export_with(Some(runtime), args, processor, send)
    }

    fn runtime(root: &std::path::Path) -> RuntimeContext {
        RuntimeContext {
            config: crate::config::Config {
                key_store: None,
                db_dir: root.join("db"),
                keys_file: root.join("keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.to_owned(),
            id: "synthetic-account".into(),
            directory: root.join("runtime"),
        }
    }

    fn args(root: &std::path::Path) -> Args {
        Args {
            output_dir: root.join("exports"),
            users: Some("alpha,beta".into()),
            incremental: false,
            start: None,
            end: None,
            dry_run: false,
            from_plan_csv: None,
            plan_mode: None,
        }
    }

    fn document(username: &str, id: i64) -> serde_json::Value {
        json!({"username":username,"chat":username,"exported_at":"synthetic",
            "messages":[{"local_id":id,"source":"message_0.db","timestamp":id,
                "content":"synthetic","custom":"retained"}]})
    }

    fn dispatch(runtime: &RuntimeContext, request: Request) -> Result<crate::ipc::Response> {
        assert_eq!(runtime.id, "synthetic-account");
        // 故意使用不存在的配置文件；编排若重新加载配置便无法完成本测试。
        assert!(!runtime.config_path.exists());
        Ok(crate::ipc::Response::ok(match request {
            Request::ExportChatList => json!({"chats":[
                {"username":"alpha","chat":"alpha","is_group":false},
                {"username":"beta","chat":"beta","is_group":false}]}),
            Request::ExportChatByUsername { username } => document(&username, 2),
            _ => panic!("unexpected request"),
        }))
    }

    fn read(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn index_failure_reports_published_artifact_without_complete_success() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let mut args = args(temp.path());
        args.users = Some("alpha".into());
        let output = args.output_dir.clone();
        let mut previous_index = None;
        let mut lock = None;
        let summary = export_with(&runtime, args, None, |runtime, request| {
            if matches!(&request, Request::ExportChatByUsername { .. }) {
                let index = output.join("_export_index.json");
                previous_index = Some(read(&index));
                lock = Some(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(index)?,
                );
            }
            dispatch(runtime, request)
        })
        .unwrap();
        assert_eq!(summary["written"], 0);
        assert_eq!(summary["failures"].as_array().unwrap().len(), 1);
        assert_eq!(summary["failures"][0]["username"], "alpha");
        assert_eq!(summary["failures"][0]["artifact_published"], true);
        assert_eq!(read(&output.join("single_alpha.json"))["username"], "alpha");
        assert_eq!(
            read(&output.join("_export_index.json")),
            previous_index.unwrap()
        );
        drop(lock);
    }

    #[test]
    fn another_runtime_cannot_merge_same_username_into_bound_directory() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let mut first = args(temp.path());
        first.users = Some("alpha".into());
        let output = first.output_dir.clone();
        export_with(&runtime, first, None, dispatch).unwrap();
        let artifact = std::fs::read(output.join("single_alpha.json")).unwrap();
        let index = std::fs::read(output.join("_export_index.json")).unwrap();
        let mut other = runtime.clone();
        other.id = "different-account-context".into();
        let mut second = args(temp.path());
        second.users = Some("alpha".into());
        second.incremental = true;
        let error = export_with(&other, second, None, |_, request| match request {
            Request::ExportChatList => Ok(crate::ipc::Response::ok(json!({"chats":[
                {"username":"alpha","chat":"alpha","is_group":false}
            ]}))),
            _ => panic!("foreign context must be rejected before reading the chat document"),
        })
        .unwrap_err();
        assert!(error.to_string().contains("运行上下文"));
        assert_eq!(
            std::fs::read(output.join("single_alpha.json")).unwrap(),
            artifact
        );
        assert_eq!(
            std::fs::read(output.join("_export_index.json")).unwrap(),
            index
        );
    }

    #[test]
    fn callback_changes_final_merged_document_before_real_publication() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let mut args = args(temp.path());
        args.incremental = true;
        args.start = Some("2".into());
        args.end = Some("2".into());
        let output = args.output_dir.clone();
        std::fs::create_dir_all(&output).unwrap();
        let old = document("alpha", 1);
        std::fs::write(output.join("single_alpha.json"), old.to_string()).unwrap();
        let mut called = Vec::new();
        let mut processor = |target: &Target, doc: &mut serde_json::Value| {
            called.push(target.username.clone());
            assert!(!doc["date_last_msg"].as_str().unwrap().is_empty());
            if target.username == "alpha" {
                assert_eq!(doc["messages"].as_array().unwrap().len(), 2);
                assert_eq!(doc["messages"][0], old["messages"][0]);
            }
            for message in doc["messages"].as_array_mut().unwrap() {
                message["transcription"] = "synthetic transcript".into();
            }
            doc["processed"] = true.into();
            Ok(())
        };
        let summary = export_with(&runtime, args, Some(&mut processor), dispatch).unwrap();
        assert_eq!(called, ["alpha", "beta"]);
        assert_eq!(summary["written"], 2);
        assert_eq!(summary["messages"], 3);
        assert_eq!(summary["added_messages"], 2);
        let saved = read(&output.join("single_alpha.json"));
        assert_eq!(saved["processed"], true);
        assert_eq!(
            saved["messages"][0]["transcription"],
            "synthetic transcript"
        );
        assert_eq!(
            saved["messages"][1]["transcription"],
            "synthetic transcript"
        );
        assert_eq!(saved["messages"][0]["custom"], "retained");
        assert_eq!(
            read(&output.join("_export_index.json"))["chats"]["alpha"]["current_file"],
            "single_alpha.json"
        );
    }

    #[test]
    fn callback_failure_preserves_old_bytes_and_continues_other_chats() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let args = args(temp.path());
        let output = args.output_dir.clone();
        std::fs::create_dir_all(&output).unwrap();
        let path = output.join("single_alpha.json");
        let old = document("alpha", 1).to_string();
        std::fs::write(&path, &old).unwrap();
        let mut processor = |target: &Target, doc: &mut serde_json::Value| {
            doc["partial"] = true.into();
            ensure!(target.username != "alpha", "synthetic callback failure");
            Ok(())
        };
        let summary = export_with(&runtime, args, Some(&mut processor), dispatch).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), old.as_bytes());
        assert_eq!(summary["written"], 1);
        assert_eq!(
            summary["failures"],
            json!([{"username":"alpha","error":"synthetic callback failure"}])
        );
        assert_eq!(read(&output.join("single_beta.json"))["partial"], true);
        assert_eq!(
            read(&output.join("_export_index.json"))["chats"]["alpha"]["last_exported_at"],
            "synthetic"
        );
    }

    #[test]
    fn none_preserves_document_and_plan_users_time_selection() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let mut args = args(temp.path());
        let plan = temp.path().join("plan.csv");
        std::fs::write(&plan, "username,export\nalpha,1\nbeta,0\n").unwrap();
        args.from_plan_csv = Some(plan);
        args.plan_mode = Some(Mode::Whitelist);
        args.start = Some("2".into());
        args.end = Some("2".into());
        let output = args.output_dir.clone();
        let summary = export_with(&runtime, args, None, dispatch).unwrap();
        assert_eq!(
            summary,
            json!({"engine":"rust","total":1,"written":1,"messages":1,"added_messages":1,"incremental":false,"failures":[]})
        );
        let mut expected = document("alpha", 2);
        TimeRange::default().apply(&mut expected).unwrap();
        assert_eq!(read(&output.join("single_alpha.json")), expected);
        assert!(!output.join("single_beta.json").exists());
    }

    #[test]
    fn identity_checks_reject_response_and_callback_before_publication() {
        for change_in_callback in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let runtime = runtime(temp.path());
            let mut args = args(temp.path());
            args.users = Some("alpha".into());
            let output = args.output_dir.clone();
            let mut processor = |_: &Target, doc: &mut serde_json::Value| {
                doc["username"] = "wrong".into();
                Ok(())
            };
            let summary = export_with(&runtime, args, Some(&mut processor), |rt, request| {
                let chat = matches!(request, Request::ExportChatByUsername { .. });
                let mut response = dispatch(rt, request)?;
                if chat && !change_in_callback {
                    response.data["username"] = "wrong".into();
                }
                Ok(response)
            })
            .unwrap();
            assert_eq!(summary["written"], 0);
            assert_eq!(summary["failures"].as_array().unwrap().len(), 1);
            assert!(!output.join("single_alpha.json").exists());
            assert_eq!(
                read(&output.join("_export_index.json")),
                json!({
                    "version": 1,
                    "chats": {},
                    "runtime_binding": {
                        "runtime_id": runtime.id,
                        "legacy_unverified": false
                    }
                })
            );
        }
    }

    #[test]
    fn dry_run_does_not_call_processor_or_create_output() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = runtime(temp.path());
        let mut args = args(temp.path());
        args.dry_run = true;
        args.users = Some("beta".into());
        let output = args.output_dir.clone();
        let mut processor = |_: &Target, _: &mut serde_json::Value| -> Result<()> {
            panic!("dry run must not process documents")
        };
        let summary = export_with(&runtime, args, Some(&mut processor), |rt, request| {
            assert!(matches!(request, Request::ExportChatList));
            dispatch(rt, request)
        })
        .unwrap();
        assert_eq!(summary["planned"], 1);
        assert_eq!(summary["chats"][0]["username"], "beta");
        assert!(!output.exists());
    }

    #[test]
    fn generated_plan_header_and_public_args_are_compatible() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            args: crate::cli::operation_args::export_chats::Args,
        }
        let defaults = Cli::try_parse_from(["wx", "out", "--dry-run"]).unwrap();
        assert!(defaults.args.from_plan_csv.is_none());
        assert!(defaults.args.plan_mode.is_none());
        assert!(!defaults.args.incremental);
        assert!(Cli::try_parse_from(["wx", "out", "--plan-mode", "whitelist"]).is_err());
        assert!(Cli::try_parse_from([
            "wx",
            "out",
            "--from-plan-csv",
            "p.csv",
            "--plan-mode",
            "whitelist"
        ])
        .is_ok());
        let bytes = crate::application::chat_export_plan::render_plan_csv(&[]).unwrap();
        assert_eq!(
            crate::application::chat_export_plan::PLAN_CSV_FIELDS.len(),
            12
        );
        assert!(Plan::read(bytes.as_slice(), Mode::Blacklist)
            .unwrap()
            .select(["peer"])
            .unwrap()
            .is_empty());
        let row = crate::application::chat_export_plan::PlanRow {
            export: "1".into(),
            index: 999,
            username: "peer".into(),
            chat_name: "误导,\"姓名\"\n换行".into(),
            chat_type: "private".into(),
            message_count: 0,
            message_body_bytes: 0,
            first_ts: None,
            last_ts: None,
            first_time: String::new(),
            last_time: String::new(),
            attachment_estimated_bytes: 0,
            attachment_scanned_bytes: None,
            total_estimated_bytes: 0,
            size_status: "ok".into(),
        };
        let bytes = crate::application::chat_export_plan::render_plan_csv(&[row]).unwrap();
        assert_eq!(
            Plan::read(bytes.as_slice(), Mode::Whitelist)
                .unwrap()
                .select(["peer", "other"])
                .unwrap(),
            &["peer".to_owned()]
        );
    }

    #[test]
    fn parses_legacy_formats_and_rejects_invalid_ranges() {
        let midnight = parse_timestamp("2025-01-01").unwrap();
        for input in [
            "2025-01-01 00:00",
            "2025-01-01T00:00:00",
            "2025-01-01 00:00:00",
        ] {
            assert_eq!(parse_timestamp(input).unwrap(), midnight);
        }
        assert_eq!(parse_timestamp(" 123 ").unwrap(), 123);
        for input in ["2025-02-30", "bad", "9223372036854775807"] {
            assert!(parse_timestamp(input).is_err());
        }
        assert!(TimeRange::parse(Some("2"), Some("1")).is_err());
        let temp = tempfile::tempdir().unwrap();
        for (start, end) in [("bad", "3"), ("3", "2"), ("1", "bad")] {
            let mut args = args(temp.path());
            args.start = Some(start.into());
            args.end = Some(end.into());
            assert!(cmd_export(args).unwrap_err().to_string().contains("时间"));
        }
        let mut invalid_plan = args(temp.path());
        invalid_plan.plan_mode = Some(Mode::Whitelist);
        assert_eq!(
            cmd_export(invalid_plan).unwrap_err().to_string(),
            "--plan-mode 需要 --from-plan-csv"
        );
        let plan = temp.path().join("invalid.csv");
        std::fs::write(&plan, "invalid_header\nvalue\n").unwrap();
        let expected = Plan::load(&plan, Mode::Whitelist)
            .err()
            .unwrap()
            .to_string();
        let mut invalid_plan = args(temp.path());
        invalid_plan.from_plan_csv = Some(plan);
        invalid_plan.plan_mode = Some(Mode::Whitelist);
        assert_eq!(cmd_export(invalid_plan).unwrap_err().to_string(), expected);
        assert!(!temp.path().join("exports").exists());
    }

    #[test]
    fn filters_inclusively_and_preserves_null_without_bounds() {
        let source = json!({"messages":[{"timestamp":null},{"timestamp":1},{"timestamp":2},{"timestamp":3}]});
        let mut all = source.clone();
        assert_eq!(TimeRange::default().apply(&mut all).unwrap(), 4);
        assert_eq!(all["date_first_msg"], "");
        let mut bounded = source.clone();
        assert_eq!(
            TimeRange::parse(Some("1"), Some("2"))
                .unwrap()
                .apply(&mut bounded)
                .unwrap(),
            2
        );
        assert_eq!(
            bounded["messages"],
            json!([{"timestamp":1},{"timestamp":2}])
        );
        assert!(!bounded["date_first_msg"].as_str().unwrap().is_empty());
        let mut empty = source;
        assert_eq!(
            TimeRange::parse(Some("4"), None)
                .unwrap()
                .apply(&mut empty)
                .unwrap(),
            0
        );
        assert_eq!(empty["date_last_msg"], "");
        assert!(TimeRange::default()
            .apply(&mut json!({"messages":null}))
            .is_err());
    }
}
