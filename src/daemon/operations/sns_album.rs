//! 固定账号的原生相册入口；仅以选定账号的 Feed 和缓存作为来源。
use super::history::{parse_time, parse_time_end};
use crate::infrastructure::output_tree as publish;
use crate::service::query_client as transport;
use crate::{
    adapters::wechat::moments::cache,
    application::moments::{album, album_render},
    ipc::Request,
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use std::{fs, path::Path, time::Duration};

pub use crate::service::operation_requests::sns_album::Args;

fn feed(
    runtime: &RuntimeContext,
    args: &Args,
    since: Option<i64>,
    until: Option<i64>,
) -> Result<crate::ipc::Response> {
    let mut last = None;
    // 保留旧只读 Feed 的六次尝试；所有尝试始终使用同一账号，不再启动 CLI 子进程。
    for attempt in 1..=6 {
        match transport::send_with_limits(
            runtime,
            Request::SnsFeed {
                limit: args.limit,
                since,
                until,
                user: Some(args.user.clone()),
            },
            Duration::from_secs(30),
            256 * 1024 * 1024,
        ) {
            Ok(response) => return Ok(response),
            Err(error) => last = Some(error),
        }
        if attempt < 6 {
            eprintln!("SNS 查询未成功，准备第 {} 次尝试", attempt + 1);
            std::thread::sleep(Duration::from_secs((attempt * 2).min(10)));
        }
    }
    Err(last.expect("six failed SNS attempts")).context("SNS 查询六次尝试均未成功")
}

fn existing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn cmd_sns_album(args: Args) -> Result<()> {
    ensure!(!args.user.trim().is_empty(), "相册作者不能为空");
    let since = args.since.as_deref().map(parse_time).transpose()?;
    let until = args.until.as_deref().map(parse_time_end).transpose()?;
    if let (Some(since), Some(until)) = (since, until) {
        ensure!(since <= until, "起始时间不能晚于结束时间");
    }
    ensure!(
        !args.adopt_existing || args.output_dir.is_some(),
        "认领旧相册需要 --output-dir"
    );
    let output = std::path::absolute(match &args.output_dir {
        Some(path) => path.clone(),
        None => args.output.join(format!(
            "{}-朋友圈相册-{}",
            album_render::safe_stem(&args.user.clone().into(), "contact"),
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    })?;
    let runtime = RuntimeContext::load()?;
    super::export_chat::validate_output_for(&runtime, &output)?;
    crate::infrastructure::publication::separate(&runtime.root, &output)?;
    let account = cache::account_root(&runtime.config.db_dir)?;
    let cache_root = cache::account_cache_root(account);
    crate::infrastructure::publication::separate(&cache_root, &output)?;

    let response = feed(&runtime, &args, since, until)?;
    let username = response.data["resolved_user"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("后台 SNS 响应缺少精确作者，请使用当前版本重启该账号后台")?
        .to_owned();
    let posts = response.data["posts"]
        .as_array()
        .context("后台 SNS 响应缺少帖子列表")?
        .clone();
    let has_videos = posts
        .iter()
        .filter_map(|post| post["media"].as_array())
        .flatten()
        .any(|media| {
            matches!(media["type"].as_str(), Some("6" | "15"))
                || matches!(media["type"].as_i64(), Some(6 | 15))
        });
    let cache = if !args.no_videos && has_videos && existing(&cache_root)? {
        Some(cache::build_video_cache_index(
            &cache_root,
            Default::default(),
        )?)
    } else {
        None
    };
    let mut inputs = Vec::new();
    for path in [
        &runtime.config_path,
        &runtime.config.keys_file,
        &runtime.config.db_dir,
        &runtime.config.decrypted_dir,
        &runtime.root,
        &cache_root,
    ] {
        if existing(path)? {
            inputs.push(path.clone());
        }
    }
    let mut warnings = Vec::new();
    if response.data["scan_truncated"].as_bool().unwrap_or(false) {
        warnings.push("SNS 数据库扫描达到上限；本相册不代表完整历史，请核对范围".into());
    }
    let options = album::Options {
        user: args.user,
        output,
        binding: publish::Binding {
            version: 1,
            tree_kind: "album".into(),
            source_kind: "account".into(),
            source_id: runtime.id,
            user_name: username,
        },
        policy: if args.adopt_existing {
            publish::ExistingPolicy::Adopt
        } else if args.output_dir.is_some() {
            publish::ExistingPolicy::Update
        } else {
            publish::ExistingPolicy::Reject
        },
        inputs,
        image_workers: args.image_workers.clamp(1, 32) as usize,
        video_workers: args.video_workers.clamp(1, 16) as usize,
        no_remote: args.no_remote,
        no_videos: args.no_videos,
        warnings,
    };
    let summary = album::export(posts, &options, cache.as_ref())?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
