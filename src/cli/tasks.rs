//! Thin CLI for the account-bound local task service.
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, time::Duration};

use crate::{
    runtime::RuntimeContext,
    service::{
        client::request,
        protocol::{Call, Format, Kind, Options, Submission, Task},
        settings::SettingsInput,
    },
};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    Configure(SettingsArgs),
    #[command(alias = "status")]
    Info,
    List,
    #[command(alias = "show")]
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
    #[arg(long)]
    pub include_voice: bool,
    #[arg(long)]
    pub include_sns: bool,
    #[arg(long)]
    pub include_sns_media: bool,
    #[arg(long)]
    pub no_images: bool,
    #[arg(long)]
    pub allow_missing_media: bool,
    #[arg(long)]
    pub with_transcriptions: bool,
    #[arg(long, requires = "with_transcriptions")]
    pub allow_upload: bool,
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
                include_voice: self.include_voice,
                include_sns: self.include_sns,
                include_sns_media: self.include_sns_media,
                include_images: !self.no_images,
                allow_missing_media: self.allow_missing_media,
                with_transcriptions: self.with_transcriptions,
                allow_upload: self.allow_upload,
                authorize_memory_scan: self.authorize_memory_scan,
            },
        }
    }

    fn validate(&self) -> Result<()> {
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
        Ok(())
    }
}

use crate::service::protocol::parse_task_kind as parse_kind;

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
        Command::Get { id } | Command::Logs { id, .. } | Command::Cancel { id } => {
            parse_id(id).map_err(anyhow::Error::msg)?;
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
    tokio::task::spawn_blocking(move || super::transport::ensure_running(&fixed))
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
        let args = submit(&["tasks", "submit", "export-all"]);
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
            "voice_mp3",
        ] {
            assert_eq!(
                parse_kind(kind).unwrap(),
                parse_kind(&kind.replace('_', "-")).unwrap()
            );
        }
        let args = submit(&[
            "tasks",
            "submit",
            "export_all",
            "--users",
            "alice,bob",
            "--formats",
            "json,csv,html",
            "--include-voice",
            "--include-sns",
            "--include-sns-media",
            "--no-images",
            "--allow-missing-media",
            "--with-transcriptions",
            "--allow-upload",
            "--wait",
        ]);
        let o = args.submission().options;
        assert_eq!(o.users, ["alice", "bob"]);
        assert_eq!(o.formats, [Format::Json, Format::Csv, Format::Html]);
        assert!(
            o.include_voice
                && o.include_sns
                && o.include_sns_media
                && o.allow_missing_media
                && o.with_transcriptions
                && o.allow_upload
                && !o.include_images
                && args.wait
        );
        for argv in [
            vec!["tasks", "submit", "wxwork-run"],
            vec!["tasks", "submit", "wxwork-export"],
            vec!["tasks", "submit", "wxwork-decrypt"],
            vec!["tasks", "submit", "wxwork-discover"],
            vec!["tasks", "submit", "wxwork-scan"],
            vec!["tasks", "submit", "export-all", "--all-conversations"],
            vec!["tasks", "submit", "shell"],
            vec!["tasks", "submit", "export-all", "--path", "x"],
            vec!["tasks", "submit", "export-all", "--argv", "x"],
            vec!["tasks", "submit", "export-all", "--formats", "xml"],
            vec!["tasks", "configure", "--port", "80"],
            vec!["tasks", "submit", "export-all", "--", "cmd.exe"],
        ] {
            assert!(Invocation::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn ids_and_scan_consent_are_checked_locally() {
        let id = "a1".repeat(32);
        for command in ["get", "show", "logs", "cancel"] {
            assert!(Invocation::try_parse_from(["tasks", command, &id]).is_ok());
            for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
                assert!(Invocation::try_parse_from(["tasks", command, bad]).is_err());
                assert!(Invocation::try_parse_from([
                    "tasks",
                    "submit",
                    "export-all",
                    "--request-id",
                    bad
                ])
                .is_err());
            }
        }
        assert_eq!(
            submit(&["tasks", "submit", "export-all", "--request-id", &id])
                .request_id
                .as_deref(),
            Some(id.as_str())
        );
        for kind in ["wechat-keys", "image-key"] {
            assert!(submit(&["tasks", "submit", kind]).validate().is_err());
            assert!(
                submit(&["tasks", "submit", kind, "--authorize-memory-scan"])
                    .validate()
                    .is_ok()
            );
        }
        assert!(
            submit(&["tasks", "submit", "export-all", "--authorize-memory-scan"])
                .validate()
                .is_err()
        );
        assert!(matches!(
            Invocation::try_parse_from(["tasks", "status"])
                .unwrap()
                .command,
            Command::Info
        ));
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
}
