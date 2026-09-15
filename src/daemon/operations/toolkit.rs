use super::output::{print_value, resolve};
use crate::service::operations::ToolkitOperation;
use crate::toolkit as native;
use crate::toolkit::legacy::{python_available, toolkit_python, toolkit_root};
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct ToolkitStatus {
    native_commands: Vec<&'static str>,
    wechat_decrypt_dir: String,
    wechat_decrypt_dir_exists: bool,
    python: String,
    python_exists: bool,
    config_json: String,
    config_json_exists: bool,
    scripts: Vec<ScriptStatus>,
    env_overrides: EnvOverrides,
}

#[derive(Serialize)]
struct ScriptStatus {
    name: &'static str,
    path: String,
    exists: bool,
}

#[derive(Serialize)]
struct EnvOverrides {
    wx_wechat_decrypt_dir: Option<String>,
    wx_wechat_decrypt_python: Option<String>,
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
            let keys = super::toolkit_run_prepare::load_saved(&runtime)?;
            super::export_emoticons::export(runtime, keys, args)
        }
        ToolkitOperation::Status { json } => cmd_status(json),
        ToolkitOperation::Decrypt {
            incremental,
            dry_run,
        } => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = super::toolkit_run_prepare::load_saved(&runtime)?;
            native::decrypt(
                &runtime,
                &keys,
                incremental,
                dry_run,
                native::DecryptMode::Strict,
            )
        }
        ToolkitOperation::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => native::decode_images(attach_dir, decoded_dir, aes_key, xor_key, force),
        ToolkitOperation::DecodeImage {
            dat_file,
            output_file,
        } => native::decode_image(dat_file, output_file),
        ToolkitOperation::BatchDecryptImages {
            input_dir,
            output_dir,
        } => native::batch_images(input_dir, output_dir),
        ToolkitOperation::VoiceToMp3 { input, output } => {
            let context = native::export_context::ExportContext::current()?;
            let output = output.unwrap_or_else(|| {
                let mut path = PathBuf::from(&input);
                path.set_extension("mp3");
                path.to_string_lossy().into_owned()
            });
            let protected = context.protected(std::path::Path::new(&input))?;
            let result = native::audio::convert_silk_to_mp3_checked(
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
            let mut options = native::audio::batch::BatchOptions::from_config_file(&config)?;
            if let Some(output) = output_dir {
                options.output_dir = output;
            }
            if let Some(contacts) = contacts {
                options.contacts = native::audio::batch::parse_contact_filter(&contacts);
            }
            // The operation worker's Job owns disconnect/cancellation cleanup.
            let report =
                native::audio::batch::convert_database_checked(&options, &[config], || false)?;
            finish_voice_batch(&report)
        }
    }
}
fn finish_voice_batch(report: &native::audio::batch::BatchReport) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    voice_batch_outcome(report).require_success()?;
    Ok(())
}

pub(super) fn voice_batch_outcome(
    report: &native::audio::batch::BatchReport,
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
        let report = native::audio::batch::BatchReport {
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
    let root = toolkit_root();
    let python = toolkit_python();
    let scripts = [
        "main.py",
        "decrypt_db.py",
        "export_all_chats.py",
        "export_sns.py",
        "export_sns_album.py",
        "sns_media_wasm/wasm_video_decode.js",
        "sns_media_wasm/wasm_video_decode.wasm",
        "sns_media_wasm/weflow_wasm_keystream.js",
        "decode_image.py",
        "batch_decrypt_images.py",
        "voice_to_mp3.py",
        "transcribe_chat.py",
        "monitor_web.py",
        "app_gui.py",
    ]
    .into_iter()
    .map(|name| {
        let path = root.join(name);
        ScriptStatus {
            name,
            path: path.to_string_lossy().into_owned(),
            exists: path.exists(),
        }
    })
    .collect();

    let config = root.join("config.json");
    let status = ToolkitStatus {
        native_commands: vec![
            "run status",
            "run decrypt",
            "run emoticons",
            "run decode-images",
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
        ],
        wechat_decrypt_dir: root.to_string_lossy().into_owned(),
        wechat_decrypt_dir_exists: root.is_dir(),
        python: python.to_string_lossy().into_owned(),
        python_exists: python_available(&python),
        config_json: config.to_string_lossy().into_owned(),
        config_json_exists: config.is_file(),
        scripts,
        env_overrides: EnvOverrides {
            wx_wechat_decrypt_dir: std::env::var("WX_WECHAT_DECRYPT_DIR").ok(),
            wx_wechat_decrypt_python: std::env::var("WX_WECHAT_DECRYPT_PYTHON").ok(),
        },
    };
    print_value(&serde_json::to_value(status)?, &resolve(json))
}
