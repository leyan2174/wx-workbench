//! Business operation implementations. Only authenticated daemon workers execute this dispatcher.
use crate::service::operations::Operation;
use anyhow::Result;
mod capabilities;
pub(crate) mod chat_plan;
pub(crate) mod cleanup_native;
pub(crate) mod database_key_validation;
pub(crate) mod database_keys;
mod decode_images;
mod decrypt_databases;
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
mod media_snapshot;
pub(crate) mod monitor_native;
pub(crate) mod new_messages;
pub(crate) mod output;
pub(crate) mod plan_tasks;
pub(crate) mod setup_native;
pub(crate) mod sns_album;
pub(crate) mod sns_archive;
pub(crate) mod sns_timeline;
pub(crate) mod sns_video;
pub(crate) mod task_worker;
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
        Operation::DecodeMomentVideo {
            input,
            output,
            key_file,
            wasm,
        } => sns_video::cmd_decode(input, output, key_file, wasm),
        Operation::ExportMomentSnapshot {
            sns_db,
            output_dir,
            contact_db,
            contacts,
            utc_offset,
            download_media,
            update,
            adopt_existing,
            local_cache,
        } => export_sns::cmd_export(export_sns::Args {
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
        Operation::ExportEmoticons(args) => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = crate::service::worker_keys::database_keys(&runtime)?
                .ok_or(crate::key_store::Error::Missing)?;
            database_key_validation::validate_paths(&runtime, &keys.0)?;
            export_emoticons::export(runtime, keys, args)
        }
        Operation::Capabilities { json } => capabilities::execute(json),
        Operation::DecryptDatabases {
            incremental,
            dry_run,
        } => decrypt_databases::execute(incremental, dry_run),
        Operation::DecodeImageCache {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => decode_images::cache(attach_dir, decoded_dir, aes_key, xor_key, force),
        Operation::DecodeImage {
            dat_file,
            output_file,
        } => decode_images::image(dat_file, output_file),
        Operation::DecodeImageDirectory {
            input_dir,
            output_dir,
        } => decode_images::directory(input_dir, output_dir),
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
