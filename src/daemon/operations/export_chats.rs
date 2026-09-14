//! 原生批量导出编排；计划 CSV 只选择 username，不串联语音转录。
use crate::runtime::RuntimeContext;
use crate::toolkit::chat_plan_selection::{Mode, Plan};
use crate::{ipc::Request, message::export::Target, toolkit::chat_index::ChatIndex};
use anyhow::{ensure, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::Serialize;
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 输出目录
    pub output_dir: PathBuf,
    /// 仅导出指定 username，逗号分隔；也可设 WECHAT_EXPORT_USERS
    #[arg(long)]
    pub users: Option<String>,
    /// 保留旧消息并按分片来源和 local_id 追加；身份歧义时拒绝覆盖
    #[arg(short = 'i', long)]
    pub incremental: bool,
    /// 起始本地时间（含端点）：日期、日期时间或 Unix 秒
    #[arg(long)]
    pub start: Option<String>,
    /// 结束本地时间（含端点）；仅日期表示当天零点
    #[arg(long)]
    pub end: Option<String>,
    /// 只列出会话，不创建输出目录或索引
    #[arg(long)]
    pub dry_run: bool,
    /// 读取计划 CSV，以 username 精确选择会话
    #[arg(long)]
    pub from_plan_csv: Option<PathBuf>,
    /// blacklist 仅跳过 export=0（默认）；whitelist 仅选择 export=1
    #[arg(long, value_enum, requires = "from_plan_csv")]
    #[serde(with = "crate::service::operations::optional_plan_mode")]
    pub plan_mode: Option<Mode>,
}

#[derive(Default)]
struct TimeRange {
    start: Option<i64>,
    end: Option<i64>,
}

pub(super) fn parse_timestamp(raw: &str) -> Result<i64> {
    let raw = raw.trim();
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    let datetime = date.or_else(|| {
        ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"]
            .into_iter()
            .find_map(|format| NaiveDateTime::parse_from_str(raw, format).ok())
    });
    if let Some(datetime) = datetime {
        // 夏令时跳变或重叠不能静默猜测；用户可改用 Unix 秒明确指定。
        return Local
            .from_local_datetime(&datetime)
            .single()
            .map(|d| d.timestamp())
            .ok_or_else(|| anyhow::anyhow!("本地时间不存在或有歧义，请使用 Unix 秒: {raw}"));
    }
    let value: i64 = raw
        .parse()
        .map_err(|_| anyhow::anyhow!("无法解析时间: {raw}"))?;
    ensure!(
        Local.timestamp_opt(value, 0).single().is_some(),
        "时间超出支持范围: {raw}"
    );
    Ok(value)
}

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
}

pub fn cmd_export(args: Args) -> Result<()> {
    let has_plan = args.from_plan_csv.is_some();
    let summary = export_with(None, args, None, super::transport::send_for)?;
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
    export_with(Some(runtime), args, processor, super::transport::send_for)
}

// 仅注入请求边界，测试仍执行真实选择、合并、回调与原子文件发布。
fn export_with(
    runtime: Option<&RuntimeContext>,
    args: Args,
    mut processor: Option<&mut DocumentProcessor<'_>>,
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
    if !raw.trim().is_empty() {
        let wanted: HashSet<_> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        targets.retain(|t| wanted.contains(t.username.as_str()));
        ensure!(!targets.is_empty(), "指定 username 列表跟会话表没有交集");
    }
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
    let mut index = ChatIndex::open(&output)?;
    let mut written = 0;
    let mut messages = 0;
    let mut added_messages = 0;
    let mut failures = Vec::new();
    for (position, target) in targets.iter().enumerate() {
        eprintln!("[{}/{}] 导出 {}", position + 1, targets.len(), target.chat);
        let result = (|| -> Result<(usize, usize)> {
            let path = index.choose(&target.username, &target.chat, target.is_group)?;
            super::export_chat::validate_output_for(runtime, &path)?;
            let destination = crate::toolkit::ExportTarget::capture(runtime, &path)?;
            let mut response = send(
                runtime,
                Request::ExportChatByUsername {
                    username: target.username.clone(),
                },
            )?;
            ensure!(
                response.data["username"].as_str() == Some(&target.username),
                "返回的聊天身份与请求不符"
            );
            let mut added = range.apply(&mut response.data)?;
            if incremental {
                let previous = index
                    .current(&target.username)?
                    .or_else(|| path.exists().then(|| path.clone()));
                if let Some(previous) = previous {
                    let old = serde_json::from_reader(std::io::BufReader::new(
                        std::fs::File::open(previous)?,
                    ))?;
                    let merged = crate::toolkit::chat_merge::merge_chat_json(&old, &response.data)?;
                    added = merged.report.added;
                    let mut document = merged.document;
                    for key in ["chat", "exported_at"] {
                        document[key] = response.data[key].clone();
                    }
                    for key in [
                        "contact_remark",
                        "contact_nick_name",
                        "contact_memo",
                        "contact_tags",
                        "metadata_warnings",
                    ] {
                        if let Some(value) = response.data.get(key) {
                            document[key] = value.clone();
                        } else {
                            document.as_object_mut().unwrap().remove(key);
                        }
                    }
                    if target.is_group {
                        document["is_group"] = true.into();
                    } else {
                        document.as_object_mut().unwrap().remove("is_group");
                    }
                    response.data = document;
                }
            }
            // 日期条件只约束本次追加，不删减原有消息；重算合并后的完整日期范围。
            let count = TimeRange::default().apply(&mut response.data)?;
            if let Some(processor) = processor.as_deref_mut() {
                processor(target, &mut response.data)?;
                ensure!(
                    response.data["username"].as_str() == Some(&target.username),
                    "处理后的聊天身份与请求不符"
                );
            }
            destination.write_json(&response.data)?;
            index.record(&path, &target.username)?;
            Ok((count, added))
        })();
        match result {
            Ok((count, added)) => {
                written += 1;
                messages += count;
                added_messages += added;
            }
            Err(error) => failures.push(Failure {
                username: target.username.clone(),
                error: error.to_string(),
            }),
        }
    }
    Ok(
        serde_json::json!({"engine":"rust","total":targets.len(),"written":written,"messages":messages,"added_messages":added_messages,"incremental":incremental,"failures":failures}),
    )
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
            assert!(!output.join("_export_index.json").exists());
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
            args: Args,
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
        let bytes = crate::toolkit::chat_plan::render_plan_csv(&[]).unwrap();
        assert_eq!(crate::toolkit::chat_plan::PLAN_CSV_FIELDS.len(), 12);
        assert!(Plan::read(bytes.as_slice(), Mode::Blacklist)
            .unwrap()
            .select(["peer"])
            .unwrap()
            .is_empty());
        let row = crate::toolkit::chat_plan::PlanRow {
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
        let bytes = crate::toolkit::chat_plan::render_plan_csv(&[row]).unwrap();
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
