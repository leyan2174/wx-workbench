//! 个人聊天目录导出入口；账号只固定一次，GUI 可直接调用 export_for。
use crate::toolkit::chat_directory::{self, Format, Options};
use crate::{ipc::Request, message::export::Target, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Debug, Deserialize)]
struct DirectoryTarget {
    #[serde(flatten)]
    target: Target,
    table_name: String,
    identity_status: String,
}

fn select_targets(
    targets: Vec<DirectoryTarget>,
    wanted: &BTreeSet<String>,
) -> Result<Vec<DirectoryTarget>> {
    let mut tables = BTreeSet::new();
    let mut usernames = BTreeSet::new();
    for entry in &targets {
        let hash = entry
            .table_name
            .strip_prefix("Msg_")
            .context("目录含非法消息表名")?;
        ensure!(
            hash.len() == 32
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "目录含非法消息表名"
        );
        ensure!(
            tables.insert(entry.table_name.clone())
                && usernames.insert(entry.target.username.clone()),
            "消息表目录包含重复表或歧义 username"
        );
        match entry.identity_status.as_str() {
            "mapped" => ensure!(
                !entry.target.username.is_empty()
                    && format!("{:x}", md5::compute(entry.target.username.as_bytes())) == hash,
                "目录 username 与消息表不符"
            ),
            "unmapped" => ensure!(
                entry.target.username == format!("unknown_{hash}") && !entry.target.is_group,
                "未映射消息表身份无效"
            ),
            _ => anyhow::bail!("未知目录身份状态"),
        }
    }
    let mut selected = BTreeMap::new();
    let mut missing = Vec::new();
    for username in wanted {
        let table = format!("Msg_{:x}", md5::compute(username.as_bytes()));
        let entry = targets
            .iter()
            .find(|entry| entry.target.username == *username)
            .or_else(|| targets.iter().find(|entry| entry.table_name == table));
        let Some(entry) = entry else {
            missing.push(username.as_str());
            continue;
        };
        ensure!(
            entry.target.username == *username || entry.identity_status == "unmapped",
            "指定 username 与消息表已有映射冲突"
        );
        ensure!(
            selected
                .insert(entry.table_name.clone(), username.clone())
                .is_none(),
            "同一消息表被多个名称选择，请只保留一个精确 username"
        );
    }
    ensure!(
        missing.is_empty(),
        "以下 username 不在消息表目录中，未静默忽略：{}",
        missing.join(", ")
    );
    let mut result = Vec::new();
    for mut entry in targets {
        if !wanted.is_empty() {
            let Some(username) = selected.remove(&entry.table_name) else {
                continue;
            };
            if username != entry.target.username {
                // 用户给出的精确 username 的 MD5 已命中真实表；不按显示名猜测身份。
                entry.target.is_group = username.ends_with("@chatroom");
                entry.target.chat = username.clone();
                entry.target.username = username;
                entry.identity_status = "explicit_username".into();
            }
        }
        result.push(entry);
    }
    result.sort_by(|a, b| a.target.username.cmp(&b.target.username));
    Ok(result)
}

pub use crate::service::operation_requests::export_messages::Args;

pub fn cmd(args: Args) -> Result<()> {
    let allow_missing = args.allow_missing_media;
    let report = export_for(&RuntimeContext::load()?, args)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
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
    export_with_sources(runtime, args, None)
}

fn export_with_sources(
    runtime: &RuntimeContext,
    args: Args,
    sources: Option<&[crate::toolkit::asr::database_media::DecryptedSource]>,
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
    let response = super::transport::send_for(runtime, Request::ExportDirectoryCatalog)?;
    ensure!(
        response.data["catalog"] == "message_tables",
        "后台未返回完整消息表目录"
    );
    let targets = select_targets(
        serde_json::from_value(response.data["chats"].clone())?,
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
    if args.dry_run {
        return Ok(
            json!({"engine":"rust","dry_run":true,"planned":planned,"failures":[],"media_issues":0}),
        );
    }
    // 复用 ASR 的固定账号快照，不从常驻后台缓存名推断源，不另写解密编排。
    let snapshot = if media_enabled && sources.is_none() && !targets.is_empty() {
        Some(
            crate::toolkit::asr::batch::prepare_snapshot(runtime)
                .context("准备聊天媒体静态快照失败；未回退到旧解密目录")?,
        )
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
            let response = super::transport::send_for(
                runtime,
                Request::ExportDirectoryByUsername {
                    username: target.username.clone(),
                },
            )?;
            let directory = output.join(chat_directory::directory_name(&target));
            match sources {
                Some(sources) => chat_directory::export_document_with_sources(
                    runtime,
                    &config,
                    &target,
                    &response.data,
                    &directory,
                    &options,
                    sources,
                ),
                None => chat_directory::export_document(
                    runtime,
                    &config,
                    &target,
                    &response.data,
                    &directory,
                    &options,
                ),
            }
        })();
        match result {
            Ok(report) => {
                media_issues += report.media_issues;
                completed.push(report);
            }
            Err(error) => {
                failures.push(json!({"username":target.username,"error":format!("{error:#}")}))
            }
        }
    }
    Ok(
        json!({"engine":"rust","output":output,"complete":failures.is_empty() && media_issues==0,
        "completed":completed,"failures":failures,"media_issues":media_issues,"media_enabled":media_enabled}),
    )
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
