use super::output::{print_value, resolve};
use crate::application::{database_decryption, image_publication, voice_batch_export};
use crate::infrastructure::audio;
use crate::service::operations::ToolkitOperation;
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct ToolkitStatus {
    native_commands: Vec<&'static str>,
    implementation: &'static str,
}

pub fn execute(cmd: ToolkitOperation) -> Result<()> {
    match cmd {
        ToolkitOperation::DecodeSnsVideo {
            input,
            output,
            key_file,
            wasm,
        } => super::sns_video::cmd_decode(input, output, key_file, wasm),
        ToolkitOperation::ExportSnsNative {
            sns_db,
            output_dir,
            contact_db,
            contacts,
            utc_offset,
            download_media,
            update,
            adopt_existing,
            local_cache,
        } => super::export_sns::cmd_export(super::export_sns::Args {
            sns_db,
            contact_db,
            output_dir,
            contacts,
            utc_offset,
            local_cache,
            download_media,
            update,
            adopt_existing,
        }),
        ToolkitOperation::ExportEmoticons(args) => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = crate::service::worker_keys::database_keys(&runtime)?
                .ok_or(crate::key_store::Error::Missing)?;
            super::database_key_validation::validate_paths(&runtime, &keys.0)?;
            super::export_emoticons::export(runtime, keys, args)
        }
        ToolkitOperation::Status { json } => cmd_status(json),
        ToolkitOperation::Decrypt {
            incremental,
            dry_run,
        } => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = crate::service::worker_keys::database_keys(&runtime)?
                .ok_or(crate::key_store::Error::Missing)?;
            super::database_key_validation::validate_paths(&runtime, &keys.0)?;
            database_decryption::decrypt(&runtime, &keys.0, incremental, dry_run)
        }
        ToolkitOperation::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => {
            let runtime = crate::runtime::RuntimeContext::for_operation()?;
            let needs_stored =
                runtime.config.key_store.is_some() && (aes_key.is_none() || xor_key.is_none());
            let stored = needs_stored
                .then(|| super::image_keys::publication_material(&runtime))
                .transpose()?;
            image_publication::decode_images_current(
                attach_dir,
                decoded_dir,
                aes_key,
                xor_key,
                force,
                stored,
            )?;
            if needs_stored {
                crate::service::worker_keys::verify_image_revision(&runtime)?;
            }
            Ok(())
        }
        ToolkitOperation::DecodeImage {
            dat_file,
            output_file,
        } => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let stored = super::image_keys::publication_material(&runtime)?;
            image_publication::decode_image_for(&runtime, dat_file, output_file, stored)?;
            crate::service::worker_keys::verify_image_revision(&runtime)
        }
        ToolkitOperation::BatchDecryptImages {
            input_dir,
            output_dir,
        } => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let stored = super::image_keys::publication_material(&runtime)?;
            image_publication::batch_images_for(&runtime, input_dir, output_dir, stored)?;
            crate::service::worker_keys::verify_image_revision(&runtime)
        }
        ToolkitOperation::VoiceToMp3 { input, output } => {
            let context = crate::application::publication_context::PublicationContext::current()?;
            let output = output.unwrap_or_else(|| {
                let mut path = PathBuf::from(&input);
                path.set_extension("mp3");
                path.to_string_lossy().into_owned()
            });
            let protected = context.protected(std::path::Path::new(&input))?;
            let result = audio::convert_silk_to_mp3_checked(
                std::path::Path::new(&input),
                std::path::Path::new(&output),
                &protected,
                || context.verify(),
            )?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        ToolkitOperation::VoiceBatch {
            config,
            output_dir,
            contacts,
        } => {
            let mut options = voice_batch_export::BatchOptions::from_config_file(&config)?;
            if let Some(output) = output_dir {
                options.output_dir = output;
            }
            if let Some(contacts) = contacts {
                options.contacts = voice_batch_export::parse_contact_filter(&contacts);
            }
            // The operation worker's Job owns disconnect/cancellation cleanup.
            let report =
                voice_batch_export::convert_database_checked(&options, &[config], || false)?;
            finish_voice_batch(&report)
        }
    }
}
fn finish_voice_batch(report: &voice_batch_export::BatchReport) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    voice_batch_outcome(report).require_success()?;
    Ok(())
}

pub(super) fn voice_batch_outcome(
    report: &voice_batch_export::BatchReport,
) -> crate::ipc::outcome::BusinessOutcome {
    use crate::business::voice_export::BatchState;
    use crate::ipc::outcome::BusinessOutcome;
    match report.progress.state() {
        BatchState::Success => BusinessOutcome::Success,
        BatchState::Partial => BusinessOutcome::Partial,
        BatchState::Failure => BusinessOutcome::Failure,
    }
}

#[test]
fn voice_batch_report_preserves_partial_classification() {
    use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
    for (converted, skipped_existing, failed, expected) in [
        (1, 0, 0, BusinessOutcome::Success),
        (1, 0, 1, BusinessOutcome::Partial),
        (0, 1, 1, BusinessOutcome::Partial),
        (0, 0, 1, BusinessOutcome::Failure),
    ] {
        let report = voice_batch_export::BatchReport {
            progress: crate::business::voice_export::BatchProgress {
                converted,
                skipped_existing,
                failed,
                filtered: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        let actual = finish_voice_batch(&report).map_or_else(
            |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
            |_| BusinessOutcome::Success,
        );
        assert_eq!(actual, expected);
    }
}

fn cmd_status(json: bool) -> Result<()> {
    let status = ToolkitStatus {
        native_commands: vec![
            "setup",
            "cleanup",
            "status",
            "progress",
            "decrypt",
            "export-all",
            "export-emoticons",
            "transcribe-database-native",
            "export-delta-native",
            "chat-plan-native",
            "transcribe-audio-native",
            "transcribe-chat-native",
            "decode-sns-video",
            "decrypt",
            "decode-image",
            "decode-images",
            "batch-decrypt-images",
            "voice-to-mp3",
            "voice-batch",
            "export-chats-native",
            "export-sns-native",
            "export-sns",
            "export-messages",
            "decrypt-sns",
            "find-image-key",
            "find-database-keys",
            "find-image-key-monitor",
            "monitor",
            "latency",
            "transcribe-chat",
            "web",
            "gui",
        ],
        implementation: "native-rust",
    };
    print_value(&serde_json::to_value(status)?, &resolve(json))
}
