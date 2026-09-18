//! Private typed worker entry. It never parses a public command or submits a task.
use crate::{
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
            dry_run,
            max_media_bytes,
            max_total_media_bytes,
        } => {
            selected(runtime, &config)?;
            let task_id = output
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("Missing task output identity"))?
                .to_owned();
            ensure!(
                crate::service::protocol::valid_task_id(&task_id)
                    && output
                        == runtime
                            .root
                            .join("web-output")
                            .join(&runtime.id)
                            .join(&task_id)
                            .join("chats"),
                "Task output identity mismatch"
            );
            super::export_messages::export_task_for(
                runtime,
                super::export_messages::Args {
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
                    dry_run,
                    max_media_bytes,
                    max_total_media_bytes,
                    ..Default::default()
                },
                &task_id,
            )
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
        Step::ExportHistory {
            config,
            output,
            request,
            since_ts,
            until_ts,
        } => {
            selected(runtime, &config)?;
            let id = output
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow::anyhow!("Missing history task identity"))?;
            super::export::export_task_for(runtime, id, request, (since_ts, until_ts), &output)
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
    }
}
