//! 个人聊天目录导出入口；账号只固定一次，GUI 可直接调用 export_for。
use crate::adapters::wechat::messages::catalog::{
    select_targets, RawDirectoryTarget as DirectoryTarget,
};
use crate::application::chat_directory::{self, Format, Options};
use crate::{ipc::Request, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

pub use crate::service::operation_requests::export_messages::Args;

pub fn cmd(args: Args) -> Result<()> {
    let allow_missing = args.allow_missing_media;
    let report = export_for(&RuntimeContext::load()?, args)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    require_report(&report, allow_missing)
}

fn require_report(report: &Value, allow_missing: bool) -> Result<()> {
    ensure!(
        report["failures"].as_array().is_none_or(Vec::is_empty),
        "部分聊天导出失败，详情见 failures"
    );
    ensure!(
        allow_missing || report["media_issues"].as_u64().unwrap_or(0) == 0,
        "导出包含缺失或失败的媒体；产物已明确标记，可用 --allow-missing-media 接受部分媒体结果"
    );
    Ok(())
}

/// 固定账号程序化入口；返回 failures/media_issues，调用者须检查，不能只判断 Result。
/// 只在此入口读取一次指定 config_path；不调用全局配置发现、不写配置。
pub fn export_for(runtime: &RuntimeContext, args: Args) -> Result<Value> {
    export_with_sources(runtime, args, None, None)
}

pub(super) fn export_task_for(runtime: &RuntimeContext, args: Args, task_id: &str) -> Result<()> {
    let allow_missing = args.allow_missing_media;
    let report = export_with_sources(runtime, args, None, Some(task_id))?;
    require_report(&report, allow_missing)
}

fn export_with_sources(
    runtime: &RuntimeContext,
    args: Args,
    sources: Option<&[crate::adapters::wechat::media::voice::DecryptedSource]>,
    task_id: Option<&str>,
) -> Result<Value> {
    let config = chat_directory::read_config(runtime)?;
    let output = match &args.output_dir {
        Some(path) => std::path::absolute(path)?,
        None => {
            let path = PathBuf::from(
                config
                    .get("output_base_dir")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .context("需要 --output-dir 或当前账号 output_base_dir 配置")?,
            );
            if path.is_absolute() {
                path
            } else {
                runtime
                    .config_path
                    .parent()
                    .context("配置缺少父目录")?
                    .join(path)
            }
        }
    };
    super::export_chat::validate_output_for(runtime, &output)?;
    let raw_formats = args
        .formats
        .clone()
        .or_else(|| std::env::var("WECHAT_EXPORT_FORMATS").ok());
    let formats = Format::parse_list(raw_formats.as_deref().unwrap_or("csv,html,json"))?;
    let contacts = args
        .contacts
        .clone()
        .or_else(|| std::env::var("WECHAT_EXPORT_CONTACTS").ok())
        .unwrap_or_default();
    let wanted: BTreeSet<String> = contacts
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let response =
        crate::service::query_client::send_for(runtime, Request::ExportDirectoryCatalog)?;
    ensure!(
        response.data["catalog"] == "message_tables",
        "后台未返回完整消息表目录"
    );
    let targets = select_targets(
        serde_json::from_value::<Vec<DirectoryTarget>>(response.data["chats"].clone())?,
        &wanted,
    )?;
    let media_enabled = if args.no_media {
        false
    } else {
        match std::env::var("WECHAT_EXPORT_IMAGES").ok().as_deref() {
            None | Some("1") => true,
            Some("0") => false,
            Some(_) => anyhow::bail!("WECHAT_EXPORT_IMAGES 只能为 0 或 1"),
        }
    };
    let options = Options {
        formats,
        media_enabled,
        update: args.update,
        max_media_bytes: args.max_media_bytes.unwrap_or(64 * 1024 * 1024),
        max_total_media_bytes: args.max_total_media_bytes.unwrap_or(2 * 1024 * 1024 * 1024),
    };
    options.validate()?;
    let planned: Vec<_> = targets
        .iter()
        .map(|entry| {
            let t = &entry.target;
            json!({"username":t.username,"chat":t.chat,
        "table_name":entry.table_name,"identity_status":entry.identity_status,
        "output":output.join(chat_directory::directory_name(t))})
        })
        .collect();
    let mut checkpoint = task_id
        .map(|id| {
            crate::daemon::tasks::artifacts::Checkpoint::start(
                runtime,
                id,
                args.dry_run,
                targets
                    .iter()
                    .map(|entry| crate::daemon::tasks::artifacts::Candidate {
                        username: entry.target.username.clone(),
                        directory: chat_directory::directory_name(&entry.target),
                    })
                    .collect(),
            )
        })
        .transpose()?;
    if args.dry_run {
        if let Some(checkpoint) = &mut checkpoint {
            checkpoint.result.finalized = true;
            checkpoint.save(runtime)?;
        }
        return Ok(
            json!({"engine":"rust","dry_run":true,"planned":planned,"failures":[],"media_issues":0}),
        );
    }
    let image_material = if media_enabled {
        crate::service::worker_keys::image_material(runtime)?
            .map(|material| (material.aes, material.xor))
    } else {
        None
    };
    // 固定账号的私有快照，不从常驻后台缓存名推断源。
    let snapshot = if media_enabled && sources.is_none() && !targets.is_empty() {
        Some(prepare_snapshot(runtime).context("准备聊天媒体静态快照失败；未回退到旧解密目录")?)
    } else {
        None
    };
    if let Some(snapshot) = &snapshot {
        ensure!(snapshot.account_id() == runtime.id, "媒体快照账号身份不符");
    }
    let sources = sources.or_else(|| snapshot.as_ref().map(|snapshot| snapshot.sources()));
    let mut completed = Vec::new();
    let mut failures = Vec::new();
    let mut media_issues = 0;
    for entry in targets {
        let target = entry.target;
        let result = (|| -> Result<_> {
            if media_enabled {
                crate::service::worker_keys::verify_image_revision(runtime)?;
            }
            let response = crate::service::query_client::send_for(
                runtime,
                Request::ExportDirectoryByUsername {
                    username: target.username.clone(),
                },
            )?;
            let directory = output.join(chat_directory::directory_name(&target));
            let report = match sources {
                Some(sources) => chat_directory::export_document_with_sources(
                    runtime,
                    &config,
                    &target,
                    &response.data,
                    &directory,
                    &options,
                    chat_directory::MediaInput::snapshot(sources, image_material),
                ),
                None => chat_directory::export_document(
                    runtime,
                    &config,
                    &target,
                    &response.data,
                    &directory,
                    &options,
                    chat_directory::MediaInput::current(image_material),
                ),
            }?;
            if media_enabled {
                crate::service::worker_keys::verify_image_revision(runtime)?;
            }
            Ok(report)
        })();
        match result {
            Ok(report) => {
                if let Some(checkpoint) = &mut checkpoint {
                    checkpoint.register_chat(runtime, &output, &target.username)?;
                    checkpoint.result.exported_chats += 1;
                    checkpoint.result.messages += report.messages as u64;
                    checkpoint.result.media_issues += report.media_issues as u64;
                }
                media_issues += report.media_issues;
                completed.push(report);
            }
            Err(error) => {
                if let Some(checkpoint) = &mut checkpoint {
                    checkpoint.result.failed_chats += 1;
                }
                failures.push(json!({"username":target.username,"error":format!("{error:#}")}))
            }
        }
        if let Some(checkpoint) = &checkpoint {
            checkpoint.save(runtime)?;
        }
    }
    if let Some(checkpoint) = &mut checkpoint {
        checkpoint.result.finalized = true;
        checkpoint.save(runtime)?;
    }
    Ok(
        json!({"engine":"rust","output":output,"complete":failures.is_empty() && media_issues==0,
        "completed":completed,"failures":failures,"media_issues":media_issues,"media_enabled":media_enabled}),
    )
}

fn prepare_snapshot(runtime: &RuntimeContext) -> Result<super::media_snapshot::Snapshot> {
    let mut keys = crate::service::worker_keys::database_keys(runtime)?
        .context("saved database keys unavailable")?;
    let materials = super::media_snapshot::DatabaseMaterials::new(std::mem::take(&mut keys.0));
    super::media_snapshot::prepare_snapshot(runtime, materials)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(username: &str, mapped: bool) -> DirectoryTarget {
        let hash = format!("{:x}", md5::compute(username.as_bytes()));
        let target = if mapped {
            username.to_owned()
        } else {
            format!("unknown_{hash}")
        };
        serde_json::from_value(json!({"username":target,"chat":target,"is_group":false,
            "table_name":format!("Msg_{hash}"),"identity_status":if mapped {"mapped"} else {"unmapped"}})).unwrap()
    }

    #[test]
    fn directory_selection_keeps_mapped_orphans_without_session_membership() {
        let wanted = BTreeSet::from(["orphan".into()]);
        let result =
            select_targets(vec![entry("normal", true), entry("orphan", true)], &wanted).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].target.username, "orphan");
    }

    #[test]
    fn directory_selection_resolves_explicit_username_by_actual_unmapped_table() {
        let wanted = BTreeSet::from(["lost@chatroom".into()]);
        let result = select_targets(
            vec![entry("normal", true), entry("lost@chatroom", false)],
            &wanted,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].target.username, "lost@chatroom");
        assert!(result[0].target.is_group);
        assert_eq!(result[0].identity_status, "explicit_username");
        assert_eq!(
            result[0].table_name,
            format!("Msg_{:x}", md5::compute(b"lost@chatroom"))
        );
    }

    #[test]
    fn directory_selection_preserves_unmapped_ids_and_refuses_unknown_names() {
        let unknown = entry("lost", false).target.username;
        let result = select_targets(
            vec![entry("normal", true), entry("lost", false)],
            &BTreeSet::from([unknown.clone()]),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].target.username, unknown);
        assert_eq!(result[0].identity_status, "unmapped");
        let all = select_targets(
            vec![entry("normal", true), entry("lost", false)],
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(all.len(), 2);
        assert!(select_targets(
            vec![entry("normal", true)],
            &BTreeSet::from(["missing".into()])
        )
        .is_err());
    }

    #[test]
    fn directory_selection_rejects_duplicate_tables_and_conflicting_selectors() {
        assert!(select_targets(
            vec![entry("normal", true), entry("normal", true)],
            &BTreeSet::new()
        )
        .is_err());
        let unknown = entry("lost", false).target.username;
        assert!(select_targets(
            vec![entry("lost", false)],
            &BTreeSet::from(["lost".into(), unknown])
        )
        .is_err());
        let mut invalid = entry("normal", true);
        invalid.target.username = "other".into();
        assert!(select_targets(vec![invalid], &BTreeSet::new()).is_err());
    }
}
