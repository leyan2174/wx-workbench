//! Thin CLI for the account-bound local task service.
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, time::Duration};

use crate::{
    runtime::RuntimeContext,
    service::{
        client::request,
        history_export,
        protocol::{Call, Format, Kind, Options, Submission, Task},
        settings::SettingsInput,
    },
};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    Configure(SettingsArgs),
    Info,
    List,
    Get {
        #[arg(value_parser = parse_id)]
        id: String,
    },
    Logs {
        #[arg(value_parser = parse_id)]
        id: String,
        /// Next sequence to read (inclusive); resume with the returned next_after.
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long)]
        follow: bool,
    },
    Cancel {
        #[arg(value_parser = parse_id)]
        id: String,
    },
    /// List verified artifacts of a terminal task in the fixed account.
    Artifacts {
        #[arg(value_parser = parse_id)]
        id: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// Read one bounded base64 block; use next_offset to resume.
    ReadArtifact {
        #[arg(value_parser = parse_id)]
        id: String,
        #[arg(value_parser = parse_id)]
        artifact_id: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long, default_value_t = 1048576, value_parser = clap::value_parser!(u32).range(1..=1048576))]
        max_bytes: u32,
    },
    Submit(SubmitArgs),
}

#[derive(Debug, Default, clap::Args)]
pub struct SettingsArgs {
    #[arg(long)]
    pub image_cache_dir: Option<PathBuf>,
}

impl From<SettingsArgs> for SettingsInput {
    fn from(args: SettingsArgs) -> Self {
        Self {
            image_cache_dir: args.image_cache_dir,
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct SubmitArgs {
    #[arg(value_parser = parse_kind)]
    pub kind: Kind,
    #[arg(long, value_delimiter = ',')]
    pub users: Vec<String>,
    #[arg(long, value_delimiter = ',', value_parser = parse_format)]
    pub formats: Vec<Format>,
    /// Single chat selector for export_history; never a filesystem path.
    #[arg(long)]
    pub chat: Option<String>,
    /// History start in host local time (date or date-time, not Unix seconds).
    #[arg(long)]
    pub since: Option<String>,
    /// History end; a date includes that day's 23:59:59.
    #[arg(long)]
    pub until: Option<String>,
    /// Positive history message limit; defaults to 500.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Single history format; defaults to markdown, separate from --formats.
    #[arg(long, value_parser = parse_history_format)]
    pub format: Option<history_export::Format>,
    #[arg(long)]
    pub include_sns: bool,
    #[arg(long)]
    pub include_sns_media: bool,
    #[arg(long)]
    pub no_images: bool,
    #[arg(long)]
    pub allow_missing_media: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=524288000))]
    pub max_media_bytes: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=17179869184))]
    pub max_total_media_bytes: Option<u64>,
    #[arg(long)]
    pub authorize_memory_scan: bool,
    #[arg(long, value_parser = parse_id)]
    pub request_id: Option<String>,
    #[arg(long)]
    pub wait: bool,
}

impl SubmitArgs {
    fn submission(&self) -> Submission {
        Submission {
            kind: self.kind,
            options: Options {
                users: self.users.clone(),
                formats: self.formats.clone(),
                include_sns: self.include_sns,
                include_sns_media: self.include_sns_media,
                include_images: !self.no_images,
                allow_missing_media: self.allow_missing_media,
                authorize_memory_scan: self.authorize_memory_scan,
                dry_run: self.dry_run,
                max_media_bytes: self.max_media_bytes,
                max_total_media_bytes: self.max_total_media_bytes,
                history_export: self.chat.as_ref().map(|chat| history_export::Request {
                    chat: chat.clone(),
                    since: self.since.clone(),
                    until: self.until.clone(),
                    limit: self.limit.unwrap_or(history_export::DEFAULT_LIMIT),
                    format: self.format.unwrap_or_default(),
                }),
            },
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.kind == Kind::ExportHistory
                || (self.chat.is_none()
                    && self.since.is_none()
                    && self.until.is_none()
                    && self.limit.is_none()
                    && self.format.is_none()),
            "--chat, --since, --until, --limit and --format require export_history"
        );
        if let Some(id) = &self.request_id {
            parse_id(id).map_err(anyhow::Error::msg)?;
        }
        if matches!(self.kind, Kind::WechatKeys | Kind::ImageKey) {
            ensure!(
                self.authorize_memory_scan,
                "This task requires --authorize-memory-scan"
            );
        }
        ensure!(
            !self.authorize_memory_scan || matches!(self.kind, Kind::WechatKeys | Kind::ImageKey),
            "This task does not accept --authorize-memory-scan"
        );
        validate_export_options(&self.submission())
    }
}

// Reject the new business options before either entry point opens account files.
pub(super) fn validate_export_options(task: &Submission) -> Result<()> {
    if task.kind == Kind::ExportHistory {
        return crate::service::plan::validate(task, &Default::default());
    }
    let o = &task.options;
    ensure!(
        o.history_export.is_none(),
        "history_export options require export_history"
    );
    let has_budget = o.max_media_bytes.is_some() || o.max_total_media_bytes.is_some();
    ensure!(
        task.kind == Kind::ExportAll || (!o.dry_run && !has_budget),
        "Dry run and media budgets require export_all"
    );
    ensure!(
        !has_budget || o.include_images,
        "Media budgets require images"
    );
    ensure!(!o.dry_run || !o.include_sns, "Dry run cannot include SNS");
    let single = o.max_media_bytes.unwrap_or(67108864);
    let total = o.max_total_media_bytes.unwrap_or(2147483648);
    ensure!(
        (1..=524288000).contains(&single),
        "Invalid media byte budget"
    );
    ensure!(
        (single..=17179869184).contains(&total),
        "Total media budget must cover one item and not exceed 16 GiB"
    );
    Ok(())
}

use crate::service::protocol::parse_task_kind as parse_kind;

fn parse_history_format(value: &str) -> std::result::Result<history_export::Format, String> {
    serde_json::from_value(Value::String(value.into()))
        .map_err(|_| "History format must be markdown, txt, json, or yaml".into())
}

fn parse_format(value: &str) -> std::result::Result<Format, String> {
    serde_json::from_value(Value::String(value.into()))
        .map_err(|_| "Format must be json, csv, or html".into())
}

fn parse_id(value: &str) -> std::result::Result<String, String> {
    if crate::service::protocol::valid_task_id(value) {
        Ok(value.into())
    } else {
        Err("ID must contain exactly 64 lowercase hexadecimal characters".into())
    }
}

fn new_id() -> Result<String> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut bytes = [0u8; 32];
    unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut bytes,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
        .ok()
        .context("Cannot generate task request ID")?;
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn cmd(mut command: Command) -> Result<()> {
    // Validate before loading account files or starting a background process.
    match &command {
        Command::Submit(args) => args.validate()?,
        Command::Get { id }
        | Command::Logs { id, .. }
        | Command::Cancel { id }
        | Command::Artifacts { id, .. } => {
            parse_id(id).map_err(anyhow::Error::msg)?;
        }
        Command::ReadArtifact {
            id, artifact_id, ..
        } => {
            parse_id(id).map_err(anyhow::Error::msg)?;
            parse_id(artifact_id).map_err(anyhow::Error::msg)?;
        }
        _ => (),
    }
    let request_id = if let Command::Submit(args) = &mut command {
        let id = match args.request_id.take() {
            Some(id) => id,
            None => new_id()?,
        };
        args.request_id = Some(id.clone());
        eprintln!("request_id={id}");
        Some(id)
    } else {
        None
    };
    let result = (|| {
        let runtime = RuntimeContext::load()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                tokio::select! {
                    result = run(&runtime, command) => result,
                    signal = tokio::signal::ctrl_c() => {
                        signal.context("Cannot listen for Ctrl+C")?;
                        anyhow::bail!("Client disconnected; no task was cancelled")
                    }
                }
            })
    })();
    match request_id {
        Some(id) => result.with_context(|| {
            format!(
            "request_id={id}; submission was not replayed; use tasks get {id} to check its outcome"
        )
        }),
        None => result,
    }
}

async fn ensure_service(runtime: &RuntimeContext) -> Result<Value> {
    let fixed = runtime.clone();
    tokio::task::spawn_blocking(move || crate::service::query_client::ensure_running(&fixed))
        .await
        .context("Daemon startup thread failed")??;
    crate::service::client::wait_ready(runtime).await
}

async fn run(runtime: &RuntimeContext, command: Command) -> Result<()> {
    // Serialized settings validation does not open any configured paths.
    if let Command::Configure(args) = command {
        let settings = SettingsInput::from(args);
        settings.validate_serialized()?;
        ensure_service(runtime).await?;
        return print_json(&request(runtime, Call::Configure { settings }).await?);
    }
    let info = ensure_service(runtime).await?;
    match command {
        Command::Info => print_json(&info),
        Command::List => print_json(&request(runtime, Call::List {}).await?),
        Command::Get { id } => print_json(&request(runtime, Call::Get { id }).await?),
        Command::Cancel { id } => print_json(&request(runtime, Call::Cancel { id }).await?),
        Command::Artifacts { id, offset, limit } => {
            ensure!(
                supports_artifacts(&info),
                "This task service does not support task_artifacts_v1"
            );
            print_json(&request(runtime, Call::TaskArtifacts { id, offset, limit }).await?)
        }
        Command::ReadArtifact {
            id,
            artifact_id,
            offset,
            max_bytes,
        } => {
            ensure!(
                supports_artifacts(&info),
                "This task service does not support task_artifacts_v1"
            );
            print_json(
                &request(
                    runtime,
                    Call::ReadTaskArtifact {
                        id,
                        artifact_id,
                        offset,
                        max_bytes,
                    },
                )
                .await?,
            )
        }
        Command::Logs { id, after, follow } => {
            let mut cursor = after;
            loop {
                let task = get_task(runtime, &id).await?;
                let page = log_page(&task, &mut cursor);
                if !follow || !page["logs"].as_array().unwrap().is_empty() || !page["gap"].is_null()
                {
                    print_json(&page)?;
                }
                if !follow || task.terminal() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
        Command::Submit(args) => {
            ensure!(
                args.kind != Kind::ExportHistory || supports_history_export(&info),
                "This task service does not support export_history"
            );
            match info.get("configured").and_then(Value::as_bool) {
                Some(false) => {
                    request(
                        runtime,
                        Call::Configure {
                            settings: SettingsInput::default(),
                        },
                    )
                    .await?;
                }
                Some(true) => (),
                None => anyhow::bail!("Task service Info lacks configured state"),
            }
            let id = args.request_id.as_ref().context("Missing request ID")?;
            let value = request(
                runtime,
                Call::Submit {
                    idempotency_key: id.clone(),
                    task: args.submission(),
                },
            )
            .await?;
            let mut task: Task = serde_json::from_value(value).context("Invalid submitted task")?;
            ensure!(task.id == *id, "Submitted task ID mismatch");
            if !args.wait {
                return print_json(&task);
            }
            let mut cursor = 0;
            loop {
                let page = log_page(&task, &mut cursor);
                if !page["logs"].as_array().unwrap().is_empty() || !page["gap"].is_null() {
                    write_json(&mut std::io::stderr().lock(), &page)?;
                }
                if task.terminal() {
                    print_json(&task)?;
                    ensure!(
                        task.status == "succeeded",
                        "Task {} ended with status {}",
                        task.id,
                        task.status
                    );
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                task = get_task(runtime, id).await?;
            }
        }
        Command::Configure(_) => unreachable!(),
    }
}

async fn get_task(runtime: &RuntimeContext, id: &str) -> Result<Task> {
    let task: Task = serde_json::from_value(request(runtime, Call::Get { id: id.into() }).await?)
        .context("Invalid task response")?;
    ensure!(task.id == id, "Task response ID mismatch");
    Ok(task)
}

pub(super) fn supports_artifacts(info: &Value) -> bool {
    info["capabilities"]["task_artifacts_v1"] == true
}

pub(super) fn supports_history_export(info: &Value) -> bool {
    info["task_kinds"].as_array().is_some_and(|kinds| {
        kinds
            .iter()
            .any(|kind| kind["kind"] == "export_history" && kind["enabled"] == true)
    })
}

fn log_page(task: &Task, cursor: &mut u64) -> Value {
    let gap = if *cursor < task.log_start_seq {
        let gap = json!({"from": *cursor, "to": task.log_start_seq, "reason": "logs_truncated"});
        eprintln!(
            "Warning: task {} logs truncated; missing sequences {}..{}",
            task.id, *cursor, task.log_start_seq
        );
        *cursor = task.log_start_seq;
        gap
    } else {
        Value::Null
    };
    let logs: Vec<_> = task.logs.iter().filter(|log| log.seq >= *cursor).collect();
    if let Some(last) = logs.last() {
        *cursor = (*cursor).max(last.seq.saturating_add(1));
    }
    json!({"id": task.id, "logs": logs, "gap": gap, "next_after": *cursor, "status": task.status})
}

fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

fn print_json(value: &impl Serialize) -> Result<()> {
    write_json(&mut std::io::stdout().lock(), value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{client::startup_race, protocol::ServiceError};
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Invocation {
        #[command(subcommand)]
        command: Command,
    }

    fn submit(argv: &[&str]) -> SubmitArgs {
        match Invocation::try_parse_from(argv).unwrap().command {
            Command::Submit(args) => args,
            _ => panic!("Expected submit"),
        }
    }

    #[test]
    fn help_and_defaults_need_no_account_io() {
        for argv in [
            vec!["tasks", "--help"],
            vec!["tasks", "submit", "--help"],
            vec!["tasks", "configure", "--help"],
        ] {
            assert_eq!(
                Invocation::try_parse_from(argv).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
        let args = submit(&["tasks", "submit", "export_all"]);
        assert_eq!(
            serde_json::to_value(args.submission().options).unwrap(),
            serde_json::to_value(Options::default()).unwrap()
        );
        assert!(args.request_id.is_none());
        assert!(!args.wait);
        let Command::Configure(args) = Invocation::try_parse_from(["tasks", "configure"])
            .unwrap()
            .command
        else {
            panic!()
        };
        assert_eq!(SettingsInput::from(args), SettingsInput::default());
    }

    #[test]
    fn kinds_and_flags_are_typed_without_fallback() {
        for kind in [
            "wechat_keys",
            "wechat_decrypt",
            "image_key",
            "export_all",
            "decode_images",
            "sns_decrypt",
        ] {
            assert!(parse_kind(kind).is_ok());
            assert!(parse_kind(&kind.replace('_', "-")).is_err());
        }
        let args = submit(&[
            "tasks",
            "submit",
            "export_all",
            "--users",
            "alice,bob",
            "--formats",
            "json,csv,html",
            "--include-sns",
            "--include-sns-media",
            "--no-images",
            "--allow-missing-media",
            "--wait",
        ]);
        let o = args.submission().options;
        assert_eq!(o.users, ["alice", "bob"]);
        assert_eq!(o.formats, [Format::Json, Format::Csv, Format::Html]);
        assert!(
            o.include_sns
                && o.include_sns_media
                && o.allow_missing_media
                && !o.include_images
                && args.wait
        );
        for argv in [
            vec!["tasks", "submit", "wxwork-run"],
            vec!["tasks", "submit", "wxwork-export"],
            vec!["tasks", "submit", "wxwork-decrypt"],
            vec!["tasks", "submit", "wxwork-discover"],
            vec!["tasks", "submit", "wxwork-scan"],
            vec!["tasks", "submit", "export_all", "--all-conversations"],
            vec!["tasks", "submit", "shell"],
            vec!["tasks", "submit", "export_all", "--path", "x"],
            vec!["tasks", "submit", "export_all", "--argv", "x"],
            vec!["tasks", "submit", "export_all", "--formats", "xml"],
            vec!["tasks", "configure", "--port", "80"],
            vec!["tasks", "submit", "export_all", "--", "cmd.exe"],
        ] {
            assert!(Invocation::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn ids_and_scan_consent_are_checked_locally() {
        let id = "a1".repeat(32);
        for command in ["get", "logs", "cancel"] {
            assert!(Invocation::try_parse_from(["tasks", command, &id]).is_ok());
            for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
                assert!(Invocation::try_parse_from(["tasks", command, bad]).is_err());
                assert!(Invocation::try_parse_from([
                    "tasks",
                    "submit",
                    "export_all",
                    "--request-id",
                    bad
                ])
                .is_err());
            }
        }
        assert_eq!(
            submit(&["tasks", "submit", "export_all", "--request-id", &id])
                .request_id
                .as_deref(),
            Some(id.as_str())
        );
        for kind in ["wechat_keys", "image_key"] {
            assert!(submit(&["tasks", "submit", kind]).validate().is_err());
            assert!(
                submit(&["tasks", "submit", kind, "--authorize-memory-scan"])
                    .validate()
                    .is_ok()
            );
        }
        assert!(
            submit(&["tasks", "submit", "export_all", "--authorize-memory-scan"])
                .validate()
                .is_err()
        );
        for removed in ["status", "show"] {
            assert!(Invocation::try_parse_from(["tasks", removed]).is_err());
        }
    }

    #[test]
    fn settings_fields_map_without_web_args_or_account_io() {
        let Command::Configure(args) =
            Invocation::try_parse_from(["tasks", "configure", "--image-cache-dir", "images"])
                .unwrap()
                .command
        else {
            panic!()
        };
        assert_eq!(
            SettingsInput::from(args),
            SettingsInput {
                image_cache_dir: Some("images".into()),
            }
        );
        for argv in [
            vec!["tasks", "configure", "--enterprise-input", "input"],
            vec![
                "tasks",
                "configure",
                "--enterprise-input",
                "input",
                "--enterprise-key-file",
                "key",
                "--enterprise-data-dir",
                "data",
            ],
            vec![
                "tasks",
                "configure",
                "--enterprise-input",
                "input",
                "--enterprise-key-file",
                "key",
                "--enterprise-keys-file",
                "keys",
            ],
        ] {
            assert!(Invocation::try_parse_from(argv).is_err());
        }
        let Command::Logs { after, follow, .. } =
            Invocation::try_parse_from(["tasks", "logs", &"a".repeat(64)])
                .unwrap()
                .command
        else {
            panic!()
        };
        assert_eq!(after, 0);
        assert!(!follow);
        let Command::Logs { after, follow, .. } = Invocation::try_parse_from([
            "tasks",
            "logs",
            &"a".repeat(64),
            "--after",
            "7",
            "--follow",
        ])
        .unwrap()
        .command
        else {
            panic!()
        };
        assert_eq!(after, 7);
        assert!(follow);
    }

    #[test]
    fn request_ids_are_random_and_retries_exclude_service_errors() {
        let first = new_id().unwrap();
        let second = new_id().unwrap();
        assert!(parse_id(&first).is_ok());
        assert!(parse_id(&second).is_ok());
        assert_ne!(first, second);
        assert!(startup_race(&anyhow::Error::from(
            std::io::Error::from_raw_os_error(231)
        )));
        assert!(startup_race(&anyhow::Error::from(std::io::Error::from(
            std::io::ErrorKind::NotFound
        ))));
        assert!(!startup_race(&anyhow::Error::from(
            ServiceError::unauthorized()
        )));
        assert!(!startup_race(&anyhow::Error::from(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        ))));
        assert!(!startup_race(&anyhow::anyhow!("invalid task reply")));
    }

    #[test]
    fn log_cursor_includes_zero_and_never_duplicates_or_rewinds() {
        let mut task: Task = serde_json::from_value(json!({
            "id": "a".repeat(64), "kind": "export_all", "status": "running", "created_at": 0,
            "started_at": null, "finished_at": null, "exit_code": null, "output_dir": "",
            "error": null, "log_start_seq": 0, "next_log_seq": 1,
            "logs": [{"seq": 0, "stream": "stdout", "text": "first"}]
        }))
        .unwrap();
        let mut cursor = 0;
        assert_eq!(
            log_page(&task, &mut cursor)["logs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(cursor, 1);
        assert!(log_page(&task, &mut cursor)["logs"]
            .as_array()
            .unwrap()
            .is_empty());
        task.logs.clear();
        task.log_start_seq = 5;
        task.next_log_seq = 5;
        assert_eq!(
            log_page(&task, &mut cursor)["gap"]["reason"],
            "logs_truncated"
        );
        assert_eq!(cursor, 5);
        assert!(log_page(&task, &mut cursor)["gap"].is_null());
        cursor = 100;
        log_page(&task, &mut cursor);
        assert_eq!(cursor, 100);
    }

    #[test]
    fn export_dry_run_and_budgets_map_without_changing_defaults() {
        let options = serde_json::to_value(
            submit(&["tasks", "submit", "export_all"])
                .submission()
                .options,
        )
        .unwrap();
        for key in ["dry_run", "max_media_bytes", "max_total_media_bytes"] {
            assert!(options.get(key).is_none());
        }
        let args = submit(&[
            "tasks",
            "submit",
            "export_all",
            "--dry-run",
            "--max-media-bytes",
            "1",
            "--max-total-media-bytes",
            "1",
        ]);
        args.validate().unwrap();
        let o = args.submission().options;
        assert!(o.dry_run);
        assert_eq!(o.max_media_bytes, Some(1));
        assert_eq!(o.max_total_media_bytes, Some(1));
        for extra in [
            vec!["--dry-run", "--include-sns"],
            vec!["--no-images", "--max-media-bytes", "1"],
            vec!["--no-images", "--max-total-media-bytes", "67108864"],
            vec!["--max-total-media-bytes", "67108863"],
            vec!["--max-media-bytes", "2", "--max-total-media-bytes", "1"],
        ] {
            let mut argv = vec!["tasks", "submit", "export_all"];
            argv.extend(extra);
            assert!(submit(&argv).validate().is_err());
        }
        for kind in ["wechat_decrypt", "decode_images", "sns_decrypt"] {
            for extra in [
                vec!["--dry-run"],
                vec!["--max-media-bytes", "1"],
                vec!["--max-total-media-bytes", "67108864"],
            ] {
                let mut argv = vec!["tasks", "submit", kind];
                argv.extend(extra);
                assert!(submit(&argv).validate().is_err());
            }
        }
        for (flag, value) in [
            ("--max-media-bytes", "0"),
            ("--max-media-bytes", "524288001"),
            ("--max-media-bytes", "-1"),
            ("--max-media-bytes", "1.5"),
            ("--max-media-bytes", "18446744073709551616"),
            ("--max-total-media-bytes", "0"),
            ("--max-total-media-bytes", "17179869185"),
        ] {
            assert!(
                Invocation::try_parse_from(["tasks", "submit", "export_all", flag, value]).is_err()
            );
        }
        submit(&[
            "tasks",
            "submit",
            "export_all",
            "--max-media-bytes",
            "524288000",
            "--max-total-media-bytes",
            "17179869184",
        ])
        .validate()
        .unwrap();
    }

    #[test]
    fn history_cli_maps_single_format_and_preserves_history_date_semantics() {
        let args = submit(&["tasks", "submit", "export_history", "--chat", "alice"]);
        args.validate().unwrap();
        let request = args.submission().options.history_export.unwrap();
        assert_eq!(request.chat, "alice");
        assert_eq!(request.limit, 500);
        assert_eq!(request.format, history_export::Format::Markdown);
        assert_eq!(request.resolved_window().unwrap(), (None, None));
        for format in ["markdown", "txt", "json", "yaml"] {
            let args = submit(&[
                "tasks",
                "submit",
                "export_history",
                "--chat",
                "alice,bob",
                "--format",
                format,
                "--since",
                "2026-09-01",
                "--until",
                "2026-09-18",
                "--limit",
                "10001",
            ]);
            args.validate().unwrap();
            let request = args.submission().options.history_export.unwrap();
            assert_eq!(request.chat, "alice,bob");
            assert_eq!(request.limit, 10001);
            assert_eq!(serde_json::to_value(request.format).unwrap(), format);
            assert_eq!(
                request.resolved_window().unwrap(),
                (
                    Some(crate::service::time::parse_time("2026-09-01 00:00:00").unwrap()),
                    Some(crate::service::time::parse_time("2026-09-18 23:59:59").unwrap())
                )
            );
        }
        assert!(serde_json::to_value(
            submit(&["tasks", "submit", "export_all"])
                .submission()
                .options
        )
        .unwrap()
        .get("history_export")
        .is_none());
    }

    #[test]
    fn history_cli_rejects_invalid_requests_before_account_io() {
        assert!(submit(&["tasks", "submit", "export_history"])
            .validate()
            .is_err());
        for extra in [
            vec!["--limit", "0"],
            vec!["--limit", "9223372036854775808"],
            vec!["--since", "123"],
            vec!["--until", "2026-02-30"],
            vec!["--since", "2026-09-19", "--until", "2026-09-18"],
            vec!["--users", "alice"],
            vec!["--formats", "json"],
            vec!["--no-images"],
            vec!["--include-sns"],
            vec!["--include-sns-media"],
            vec!["--allow-missing-media"],
            vec!["--authorize-memory-scan"],
            vec!["--dry-run"],
            vec!["--max-media-bytes", "1"],
            vec!["--max-total-media-bytes", "67108864"],
        ] {
            let mut argv = vec!["tasks", "submit", "export_history", "--chat", "alice"];
            argv.extend(extra);
            assert!(submit(&argv).validate().is_err(), "{argv:?}");
        }
        for chat in [
            "".to_owned(),
            " ".to_owned(),
            "a\nb".to_owned(),
            "a".repeat(257),
            "中".repeat(86),
        ] {
            assert!(
                submit(&["tasks", "submit", "export_history", "--chat", &chat])
                    .validate()
                    .is_err()
            );
        }
        for (flag, value) in [
            ("--chat", "alice"),
            ("--since", "2026-09-01"),
            ("--until", "2026-09-18"),
            ("--limit", "500"),
            ("--format", "json"),
        ] {
            assert!(submit(&["tasks", "submit", "export_all", flag, value])
                .validate()
                .is_err());
        }
        for (flag, value) in [
            ("--format", "html"),
            ("--limit", "1.5"),
            ("--limit", "-1"),
            ("--output", "C:/private"),
            ("--path", "C:/private"),
            ("--command", "cmd.exe"),
        ] {
            assert!(Invocation::try_parse_from([
                "tasks",
                "submit",
                "export_history",
                "--chat",
                "alice",
                flag,
                value
            ])
            .is_err());
        }
    }

    #[test]
    fn history_service_support_requires_an_enabled_matching_kind() {
        for info in [
            json!({}),
            json!({"capabilities":{"task_artifacts_v1":true}}),
            json!({"task_kinds":null}),
            json!({"task_kinds":{"export_history":true}}),
            json!({"task_kinds":[{"kind":"export_all","enabled":true}]}),
            json!({"task_kinds":[{"kind":"export_history","enabled":false}]}),
            json!({"task_kinds":[{"kind":"export_history","enabled":"true"}]}),
        ] {
            assert!(!supports_history_export(&info));
        }
        assert!(supports_history_export(&json!({"task_kinds":[
            {"kind":"export_all","enabled":true},{"kind":"export_history","enabled":true}
        ]})));
    }

    #[test]
    fn artifact_commands_accept_only_ids_and_bounded_numeric_options() {
        let id = "a".repeat(64);
        let artifact = "b".repeat(64);
        let command = Invocation::try_parse_from(["tasks", "artifacts", &id])
            .unwrap()
            .command;
        assert!(
            matches!(command, Command::Artifacts { id:actual, offset:0, limit:50 } if actual == id)
        );
        let command = Invocation::try_parse_from(["tasks", "read-artifact", &id, &artifact])
            .unwrap()
            .command;
        assert!(
            matches!(command, Command::ReadArtifact { id:actual, artifact_id:a, offset:0, max_bytes:1048576 } if actual == id && a == artifact)
        );
        let command = Invocation::try_parse_from([
            "tasks",
            "read-artifact",
            &id,
            &artifact,
            "--offset",
            "1048576",
            "--max-bytes",
            "1",
        ])
        .unwrap()
        .command;
        assert!(matches!(
            command,
            Command::ReadArtifact {
                offset: 1048576,
                max_bytes: 1,
                ..
            }
        ));
        for argv in [
            vec!["tasks", "artifacts", &id, "--limit", "0"],
            vec!["tasks", "artifacts", &id, "--limit", "101"],
            vec!["tasks", "artifacts", &id, "--offset", "-1"],
            vec!["tasks", "artifacts", "../other"],
            vec!["tasks", "artifacts", &id, "--path", "C:/private"],
            vec!["tasks", "read-artifact", &id, "C:/private"],
            vec!["tasks", "read-artifact", &id, &artifact, "--max-bytes", "0"],
            vec![
                "tasks",
                "read-artifact",
                &id,
                &artifact,
                "--max-bytes",
                "1048577",
            ],
            vec![
                "tasks",
                "read-artifact",
                &id,
                &artifact,
                "--max-bytes",
                "true",
            ],
            vec!["tasks", "read-artifact", &id, &artifact, "--offset", "1.5"],
            vec![
                "tasks",
                "read-artifact",
                &id,
                &artifact,
                "--output",
                "C:/private",
            ],
            vec![
                "tasks",
                "read-artifact",
                &id,
                &artifact,
                "--command",
                "cmd.exe",
            ],
        ] {
            assert!(Invocation::try_parse_from(argv).is_err());
        }
        for command in ["artifacts", "read-artifact"] {
            assert_eq!(
                Invocation::try_parse_from(["tasks", command, "--help"])
                    .unwrap_err()
                    .kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
        for info in [
            json!({}),
            json!({"capabilities":[]}),
            json!({"capabilities":{"task_artifacts_v1":false}}),
            json!({"capabilities":{"task_artifacts_v1":"true"}}),
        ] {
            assert!(!supports_artifacts(&info));
        }
        assert!(supports_artifacts(
            &json!({"capabilities":{"task_artifacts_v1":true}})
        ));
    }
}
