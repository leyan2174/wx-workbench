//! Private typed worker entry. It never parses a public command or submits a task.
use crate::{
    runtime::RuntimeContext,
    service::{plan::*, protocol::MAX_REQUEST_BYTES},
    toolkit,
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
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let step: Step = serde_json::from_slice(&bytes)?;
    execute(&runtime, step)
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

fn memory(pids: Vec<u32>, authorized: bool) -> super::enterprise_batch::MemoryArgs {
    super::enterprise_batch::MemoryArgs {
        authorize_memory_scan: authorized,
        pid: pids,
        scan_bare_hex: false,
        no_cipher_structs: false,
        scan_timeout: 120,
        scan_max_mib: 4096,
    }
}

fn keys(keys: EnterpriseKeys) -> super::enterprise_batch::KeyArgs {
    super::enterprise_batch::KeyArgs {
        key_file: keys.key_file,
        keys_file: keys.keys_file,
        auto_keys: keys.authorize_memory_scan,
        memory: memory(keys.pids, keys.authorize_memory_scan),
    }
}

fn selection(selection: EnterpriseSelection) -> Result<super::enterprise_batch::SelectionArgs> {
    ensure!(
        selection.all_conversations == selection.users.is_empty(),
        "Explicit enterprise scope required"
    );
    Ok(super::enterprise_batch::SelectionArgs {
        conversations: selection.users,
        self_id: selection.self_id,
        formats: selection
            .formats
            .into_iter()
            .map(|format| match format {
                crate::service::protocol::Format::Json => super::enterprise_batch::Format::Json,
                crate::service::protocol::Format::Csv => super::enterprise_batch::Format::Csv,
                crate::service::protocol::Format::Html => super::enterprise_batch::Format::Html,
            })
            .collect(),
    })
}

fn enterprise(command: super::enterprise_batch::Command) -> Result<()> {
    super::enterprise_batch::cmd(super::enterprise_batch::Args { command })
}

fn execute(runtime: &RuntimeContext, step: Step) -> Result<()> {
    use super::enterprise_batch as enterprise;
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
            let keys = super::toolkit_run_prepare::load_saved(runtime)?;
            toolkit::decrypt(runtime, &keys, false, false, toolkit::DecryptMode::Strict)
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
                    plan_mode: toolkit::chat_plan_selection::Mode::Blacklist,
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
            toolkit::decode_images(
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
            )
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
            let mut options = toolkit::audio::batch::BatchOptions::from_config_file(&config)?;
            options.output_dir = output;
            options.contacts = toolkit::audio::batch::parse_contact_filter(&users.join(","));
            let report = toolkit::audio::batch::convert_database(&options)?;
            emit(&report)?;
            ensure!(report.failed == 0, "Some voice conversions failed");
            Ok(())
        }
        Step::EnterpriseDecrypt {
            input,
            output,
            key_file,
        } => super::enterprise::cmd_decrypt(input, output, key_file),
        Step::EnterpriseBatchDecrypt {
            data_dir,
            output,
            keys: options,
        } => enterprise(enterprise::Command::Decrypt(enterprise::DecryptArgs {
            data_dir,
            output,
            keys: keys(options),
        })),
        Step::EnterpriseExport {
            snapshot,
            output,
            selection: options,
        } => enterprise(enterprise::Command::Export(enterprise::ExportArgs {
            snapshot,
            output,
            selection: selection(options)?,
        })),
        Step::EnterpriseDiscover { root } => enterprise(enterprise::Command::Discover { root }),
        Step::EnterpriseScan {
            data_dir,
            pids,
            authorize_memory_scan,
        } => enterprise(enterprise::Command::Scan(enterprise::ScanArgs {
            data_dir,
            keys_output: None,
            memory: memory(pids, authorize_memory_scan),
        })),
        Step::EnterpriseRun {
            data_dir,
            decrypted_output,
            export_output,
            keys: key_options,
            selection: options,
        } => enterprise(enterprise::Command::Run(enterprise::RunArgs {
            data_dir,
            decrypted_output,
            export_output,
            keys: keys(key_options),
            selection: selection(options)?,
        })),
    }
}
