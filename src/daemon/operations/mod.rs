//! Business operation implementations. Only authenticated daemon workers execute this dispatcher.
use crate::service::operations::Operation;
use anyhow::Result;
pub(crate) mod asr;
pub(crate) mod asr_batch;
pub(crate) mod asr_database;
pub(crate) mod chat_plan;
pub(crate) mod cleanup_native;
pub(crate) mod database_key_validation;
pub(crate) mod database_keys;
pub(crate) mod export;
pub(crate) mod export_all;
pub(crate) mod export_chat;
pub(crate) mod export_chats;
pub(crate) mod export_delta;
pub(crate) mod export_emoticons;
pub(crate) mod export_messages;
pub(crate) mod export_sns;
mod extract;
pub(crate) mod history;
pub(crate) mod image_key_sample;
pub(crate) mod image_keys;
pub(crate) mod init;
pub(crate) mod monitor_native;
pub(crate) mod new_messages;
pub(crate) mod output;
pub(crate) mod setup_native;
pub(crate) mod sns_album;
pub(crate) mod sns_archive;
pub(crate) mod sns_timeline;
pub(crate) mod sns_video;
pub(crate) mod task_worker;
pub(crate) mod toolkit;
pub(crate) mod voices;

pub(crate) fn execute(operation: Operation) -> Result<()> {
    operation.validate_request()?;
    match operation {
        Operation::Extract {
            attachment_id,
            output,
            overwrite,
            json,
        } => extract::execute(attachment_id, output, overwrite, json),
        Operation::TranscribeAudio { args } => asr::cmd_transcribe_audio_native(args),
        Operation::TranscribeChat { args } => asr::cmd_transcribe_chat_native(args),
        Operation::TranscribeBatch { args } => asr_batch::cmd(args),
        Operation::TranscribeDatabase { args } => {
            asr_database::cmd_transcribe_database_native(args)
        }
        Operation::ChatPlan { args } => chat_plan::cmd(args),
        Operation::Cleanup { args } => cleanup_native::cmd(args),
        Operation::DatabaseKeys { args } => database_keys::cmd(args),
        Operation::Export {
            chat,
            since,
            until,
            limit,
            format,
            output,
            opts,
        } => export::cmd_export(chat, since, until, limit, format, output, opts),
        Operation::ExportChat { chat, output } => export_chat::cmd_export(chat, output),
        Operation::ExportChats { args } => export_chats::cmd_export(args),
        Operation::ExportDelta { args } => export_delta::cmd(args),
        Operation::ExportMessages { args } => export_messages::cmd(args),
        Operation::ImageKeys { args } => image_keys::cmd(args),
        Operation::ImageKeyMonitor { args } => image_keys::cmd_monitor(args),
        Operation::Initialize {
            force,
            db_dir_override,
            provider,
            restart,
            executable,
            timeout,
        } => init::cmd_init(
            force,
            db_dir_override,
            provider,
            restart,
            executable,
            timeout,
        ),
        Operation::Monitor { args } => monitor_native::cmd_monitor(args),
        Operation::Latency { args } => monitor_native::cmd_latency(args),
        Operation::NewMessages { limit, opts } => new_messages::cmd_new_messages(limit, opts),
        Operation::Setup { args } => setup_native::cmd(args),
        Operation::SetupPreview { args } => setup_native::preview(args),
        Operation::SetupApply { args, review } => setup_native::apply_reviewed(args, &review),
        Operation::SnsAlbum { args } => sns_album::cmd_sns_album(args),
        Operation::SnsArchive { args } => sns_archive::cmd(args),
        Operation::SnsTimeline { args } => sns_timeline::cmd(args),
        Operation::Voices {
            chat,
            output,
            limit,
            offset,
            since,
            until,
            overwrite,
            json_output,
        } => voices::cmd_voices(voices::Args {
            chat,
            output,
            limit,
            offset,
            since,
            until,
            overwrite,
            json: json_output,
        }),
        Operation::Toolkit { operation } => toolkit::execute(operation),
        Operation::ExportAll { args } => {
            args.validate()?;
            let runtime = crate::runtime::RuntimeContext::load()?;
            export_all::emit(export_all::export_for(&runtime, args)?)?;
            Ok(())
        }
        Operation::RunStatus { exported_dir, json } => {
            let status = crate::application::run_status::inspect(
                &crate::config::find_config_file()?,
                exported_dir.as_deref(),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print!("{}", status.render());
            }
            Ok(())
        }
    }
}
