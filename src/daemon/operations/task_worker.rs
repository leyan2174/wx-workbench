//! Private typed worker entry. It never parses a public command or submits a task.
use crate::{
    application::voice_batch_export,
    runtime::RuntimeContext,
    service::{plan::*, protocol::MAX_REQUEST_BYTES},
};
use anyhow::{ensure, Result};
use std::{io::Read, path::Path};

pub(crate) fn run() -> Result<()> {
    // Descendant wx processes must use their normal query entry, not inherit worker mode.
    std::env::remove_var("WX_DAEMON_TASK_WORKER");
    let runtime = RuntimeContext::load()?;
    ensure!(
        std::env::var("WX_CLI_EXPECTED_RUNTIME").ok().as_deref() == Some(&runtime.id),
        "Worker account identity mismatch"
    );
    let mut input = std::io::stdin().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    ensure!(
        length > 0 && length <= MAX_REQUEST_BYTES,
        "Worker request exceeds limit"
    );
    let mut bytes = zeroize::Zeroizing::new(vec![0; length]);
    input.read_exact(&mut bytes)?;
    let request: crate::service::worker_keys::Input<Step> = serde_json::from_slice(&bytes)?;
    drop(bytes);
    let mut trailing = [0u8; 1];
    ensure!(input.read(&mut trailing)? == 0, "Unexpected worker input");
    drop(input);
    let access = crate::service::worker_keys::install(request.access)?;
    let result = execute(&runtime, request.operation);
    drop(access);
    crate::daemon::operation_worker::finish(result)
}

fn selected(runtime: &RuntimeContext, config: &Path) -> Result<()> {
    ensure!(
        config == runtime.config_path,
        "Worker configuration mismatch"
    );
    Ok(())
}

fn emit(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn execute(runtime: &RuntimeContext, step: Step) -> Result<()> {
    match step {
        Step::WechatKeys {
            config,
            authorize_memory_scan,
        } => {
            selected(runtime, &config)?;
            super::database_keys::cmd(super::database_keys::Args {
                authorize_memory_scan,
            })
        }
        Step::WechatDecrypt { config } => {
            selected(runtime, &config)?;
            let keys = crate::service::worker_keys::database_keys(runtime)?
                .ok_or_else(|| anyhow::anyhow!("Worker database key access not installed"))?;
            super::database_key_validation::validate_keys(runtime, &keys.0)?;
            crate::application::database_decryption::decrypt(runtime, &keys.0, false, false)
        }
        Step::ImageKey {
            config,
            authorize_memory_scan,
            timeout,
            max_mib,
        } => {
            selected(runtime, &config)?;
            super::image_keys::cmd(super::image_keys::Args {
                sample: Default::default(),
                authorize_memory_scan,
                offline: false,
                no_save: false,
                timeout,
                max_mib: max_mib.try_into()?,
            })
        }
        Step::ExportMessages {
            config,
            output,
            users,
            formats,
            include_images,
            allow_missing_media,
        } => {
            selected(runtime, &config)?;
            super::export_messages::cmd(super::export_messages::Args {
                output_dir: Some(output),
                contacts: Some(users.join(",")),
                formats: Some(
                    formats
                        .iter()
                        .map(|format| format.extension())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                no_media: !include_images,
                allow_missing_media,
                ..Default::default()
            })
        }
        Step::TranscribeChats {
            config,
            output,
            users,
            allow_upload,
        } => {
            selected(runtime, &config)?;
            let report = super::export_all::export_for(
                runtime,
                super::export_all::Args {
                    output_dir: Some(output),
                    with_transcriptions: true,
                    write_plan_csv: None,
                    from_plan_csv: None,
                    plan_mode: crate::service::operation_requests::plan::Mode::Blacklist,
                    size_mode: super::chat_plan::Mode::Estimate,
                    incremental: false,
                    delta_only: false,
                    start: None,
                    end: None,
                    dry_run: false,
                    users: Some(users.join(",")),
                    asr: super::asr_batch::BatchArgs {
                        explicit_backend: false,
                        asr_cache_name: "batch-transcriptions.json".into(),
                        backend: super::asr::BackendArgs {
                            allow_upload,
                            ..Default::default()
                        },
                    },
                },
            )?;
            super::export_all::emit(report)
        }
        Step::DecodeImages { config, output } => {
            selected(runtime, &config)?;
            let stored = super::image_keys::publication_material(runtime)?;
            crate::application::image_publication::decode_images_for(
                runtime,
                None,
                Some(
                    output
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid output path"))?
                        .into(),
                ),
                None,
                None,
                false,
                Some(stored),
            )?;
            crate::service::worker_keys::verify_image_revision(runtime)
        }
        Step::SnsArchive { config, output } => {
            selected(runtime, &config)?;
            super::sns_archive::cmd(super::sns_archive::Args {
                output_dir: Some(output),
                adopt_existing: false,
            })
        }
        Step::SnsExport {
            config,
            output,
            users,
            download_media,
        } => {
            selected(runtime, &config)?;
            super::sns_timeline::cmd(super::sns_timeline::Args {
                contacts: Some(users.join(",")),
                output_dir: Some(output),
                adopt_existing: false,
                download_media,
                no_remote: !download_media,
            })
        }
        Step::VoiceBatch {
            config,
            output,
            users,
        } => {
            selected(runtime, &config)?;
            let mut options = voice_batch_export::BatchOptions::from_config_file(&config)?;
            options.output_dir = output;
            options.contacts = voice_batch_export::parse_contact_filter(&users.join(","));
            // Parent Job termination remains the worker's cancellation mechanism.
            let report = voice_batch_export::convert_database_checked(
                &options,
                &crate::infrastructure::publication::export_protected(runtime),
                || false,
            )?;
            emit(&report)?;
            super::toolkit::voice_batch_outcome(&report).require_success()?;
            Ok(())
        }
    }
}
