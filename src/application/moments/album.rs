//! 相册媒体编排：显式输入和来源绑定，不发现账号、不启动 Python 或 Node。
use super::{album_images, album_render, album_videos};
use crate::adapters::wechat::media::sns_keystream::SnsKeystream;
use crate::adapters::wechat::moments::cache;
use crate::attachment::local_files::HostOutputGuard;
use crate::infrastructure::output_tree as publish;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        OnceLock,
    },
};

pub(crate) struct Options {
    pub user: String,
    pub output: PathBuf,
    pub binding: publish::Binding,
    pub policy: publish::ExistingPolicy,
    pub inputs: Vec<PathBuf>,
    pub image_workers: usize,
    pub video_workers: usize,
    pub no_remote: bool,
    pub no_videos: bool,
    pub warnings: Vec<String>,
}

struct Job<'a> {
    post_index: usize,
    media_index: usize,
    stem: String,
    post: &'a Value,
    media: &'a Value,
}

#[derive(Clone, Copy)]
enum Status {
    ImageExisting,
    ImageRemote,
    ImageMissing,
    VideoExisting,
    VideoCache,
    VideoRemote,
    VideoPartial,
    VideoMissing,
}

struct MediaResult {
    post_index: usize,
    media_index: usize,
    fields: Map<String, Value>,
    status: Status,
    staged: Option<(PathBuf, PathBuf)>,
}

type Engine = OnceLock<Option<SnsKeystream>>;

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn result(
    job: &Job<'_>,
    status: Status,
    fields: Value,
    staged: Option<(PathBuf, PathBuf)>,
) -> MediaResult {
    MediaResult {
        post_index: job.post_index,
        media_index: job.media_index,
        fields: fields
            .as_object()
            .expect("internal media result object")
            .clone(),
        status,
        staged,
    }
}

fn image_job(
    job: &Job<'_>,
    existing: &HostOutputGuard,
    staging: &HostOutputGuard,
    no_remote: bool,
    engine: &Engine,
) -> Result<MediaResult> {
    let name = format!("{}.jpg", job.stem);
    if let Some(outcome) = album_images::reuse_existing_image(&name, existing)? {
        return Ok(result(
            job,
            Status::ImageExisting,
            json!({
                "local_file":format!("images/{}", outcome.filename), "image_source":"existing"
            }),
            None,
        ));
    }
    if no_remote {
        return Ok(result(
            job,
            Status::ImageMissing,
            json!({"image_error":"remote download disabled"}),
            None,
        ));
    }
    let mut errors = Vec::new();
    for (url, key, token) in [
        ("url", "url_key", "url_token"),
        ("thumb", "thumb_key", "thumb_token"),
    ] {
        let outcome = album_images::download_sns_image(
            field(job.media, url),
            field(job.media, key),
            field(job.media, token),
            &name,
            staging,
            || {
                engine
                    .get_or_init(|| SnsKeystream::bundled(Default::default()).ok())
                    .as_ref()
                    .ok_or(album_images::ImageError::EngineUnavailable)
            },
        );
        match outcome {
            Ok(outcome) => {
                let relative = PathBuf::from("images").join(&outcome.filename);
                return Ok(result(
                    job,
                    Status::ImageRemote,
                    json!({
                        "local_file":format!("images/{}", outcome.filename),
                        "image_source":outcome.source.as_str()
                    }),
                    Some((relative, staging.output_root().join(outcome.filename))),
                ));
            }
            Err(error) => {
                if error.errors.contains(&album_images::ImageError::Output) {
                    bail!("相册图片暂存输出被拒绝");
                }
                errors.push(error.to_string());
            }
        }
    }
    Ok(result(
        job,
        Status::ImageMissing,
        json!({"image_error":errors.join("; ")}),
        None,
    ))
}

fn video_result(
    job: &Job<'_>,
    status: Status,
    outcome: album_videos::Outcome,
    staging: Option<&HostOutputGuard>,
    error: Option<String>,
) -> MediaResult {
    let mut fields = json!({
        "local_file":format!("videos/{}", outcome.filename),
        "video_source":outcome.source.as_str(), "video_complete":outcome.complete,
    });
    if matches!(
        outcome.source,
        album_videos::VideoSource::Remote | album_videos::VideoSource::Existing
    ) {
        fields["video_bytes"] = outcome.bytes.into();
    }
    if let Some(error) = error {
        fields["video_error"] = error.into();
    }
    let staged = staging.map(|guard| {
        (
            PathBuf::from("videos").join(&outcome.filename),
            guard.output_root().join(&outcome.filename),
        )
    });
    result(job, status, fields, staged)
}

fn video_job(
    job: &Job<'_>,
    existing: &HostOutputGuard,
    staging: &HostOutputGuard,
    index: Option<&cache::CacheIndex>,
    no_remote: bool,
    engine: &Engine,
) -> Result<MediaResult> {
    let name = format!("{}.mp4", job.stem);
    if let Some(outcome) = album_videos::reuse_existing_video(&name, existing)? {
        return Ok(video_result(
            job,
            Status::VideoExisting,
            outcome,
            None,
            None,
        ));
    }
    let cached = index.and_then(|index| {
        cache::find_cached_video(
            index,
            field(job.post, "post_id"),
            job.post["tid"].as_i64(),
            field(job.media, "id"),
        )
    });
    let mut error = None;
    if let Some(entry) = cached {
        match album_videos::copy_cached_video(&entry.path, &name, staging, false) {
            Ok(Some(outcome)) => {
                return Ok(video_result(
                    job,
                    Status::VideoCache,
                    outcome,
                    Some(staging),
                    None,
                ))
            }
            Ok(None) => (),
            Err(album_videos::VideoError::Output) => bail!("相册视频暂存输出被拒绝"),
            Err(e) => error = Some(e.to_string()),
        }
    }
    if !no_remote {
        match album_videos::download_video(
            field(job.media, "url").trim(),
            field(job.media, "enc_key").trim(),
            &name,
            staging,
            || {
                engine
                    .get_or_init(|| SnsKeystream::bundled(Default::default()).ok())
                    .as_ref()
                    .ok_or(album_videos::VideoError::EngineUnavailable)
            },
        ) {
            Ok(outcome) => {
                return Ok(video_result(
                    job,
                    Status::VideoRemote,
                    outcome,
                    Some(staging),
                    None,
                ))
            }
            Err(album_videos::VideoError::Output) => bail!("相册视频暂存输出被拒绝"),
            Err(e) => error = Some(e.to_string()),
        }
    }
    // 完整缓存未命中且远端未成功时，才允许保留可解码的部分缓存。
    if let Some(entry) = cached {
        match album_videos::copy_cached_video(&entry.path, &name, staging, true) {
            Ok(Some(outcome)) => {
                let status = if outcome.complete {
                    Status::VideoCache
                } else {
                    Status::VideoPartial
                };
                return Ok(video_result(job, status, outcome, Some(staging), error));
            }
            Ok(None) => (),
            Err(album_videos::VideoError::Output) => bail!("相册视频暂存输出被拒绝"),
            Err(e) => error = Some(e.to_string()),
        }
    }
    Ok(result(
        job,
        Status::VideoMissing,
        json!({
            "video_error":error.unwrap_or_else(|| if no_remote { "remote download disabled".into() } else { "video unavailable".into() })
        }),
        None,
    ))
}

fn run_jobs(
    jobs: &[Job<'_>],
    workers: usize,
    process: impl Fn(&Job<'_>, &Engine) -> Result<MediaResult> + Sync,
) -> Result<Vec<MediaResult>> {
    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers.max(1).min(jobs.len()) {
            handles.push(scope.spawn(|| {
                let engine = Engine::new();
                let mut results = Vec::new();
                while !cancelled.load(Ordering::Acquire) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(index) else { break };
                    match process(job, &engine) {
                        Ok(result) => results.push(result),
                        Err(error) => {
                            cancelled.store(true, Ordering::Release);
                            return Err(error);
                        }
                    }
                }
                Ok::<_, anyhow::Error>(results)
            }));
        }
        let mut results = Vec::with_capacity(jobs.len());
        let mut failure = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(items)) => results.extend(items),
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    cancelled.store(true, Ordering::Release);
                    failure.get_or_insert_with(|| anyhow::anyhow!("相册媒体工作线程异常退出"));
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        results.sort_by_key(|item| (item.post_index, item.media_index));
        Ok(results)
    })
}

fn legacy_identity(root: &Path, user: &str, username: &str) -> Result<()> {
    if let Some(value) = publish::read_legacy_json(&root.join("timeline.json"), 256 * 1024 * 1024)?
    {
        let posts = value
            .as_array()
            .context("旧相册 timeline.json 必须是列表，不能认领时间线目录")?;
        for post in posts {
            let author = field(post, "author_username");
            ensure!(
                author.is_empty() || author == username,
                "旧相册存在其他联系人记录，拒绝认领"
            );
        }
        if posts
            .iter()
            .any(|post| !field(post, "author_username").is_empty())
        {
            return Ok(());
        }
    }
    if let Some(value) = publish::read_legacy_json(&root.join("export_summary.json"), 1024 * 1024)?
    {
        let previous_user = field(&value, "user");
        ensure!(
            previous_user.is_empty() || previous_user == user || previous_user == username,
            "旧相册摘要的联系人不匹配"
        );
    }
    Ok(())
}

/// timeline 保持旧数组形状；媒体失败计入摘要，输出或身份保护失败则停止发布。
pub(crate) fn export(
    mut posts: Vec<Value>,
    options: &Options,
    cache: Option<&cache::CacheIndex>,
) -> Result<Value> {
    ensure!(options.binding.tree_kind == "album", "相册来源类型不匹配");
    let mut image_jobs = Vec::new();
    let mut video_jobs = Vec::new();
    let mut targets = vec![
        PathBuf::from("timeline.json"),
        PathBuf::from("timeline.html"),
        PathBuf::from("export_summary.json"),
    ];
    // 只接受原始 Feed；绝不沿用输入伪造或另一目录留下的 local_file。
    for post in &mut posts {
        ensure!(post.is_object(), "SNS Feed 帖子必须是对象");
        ensure!(
            field(post, "author_username") == options.binding.user_name,
            "SNS Feed 作者与来源绑定不匹配"
        );
        let media = post
            .get_mut("media")
            .and_then(Value::as_array_mut)
            .context("SNS Feed 媒体必须是列表")?;
        for item in media {
            let object = item
                .as_object_mut()
                .context("SNS Feed 媒体条目必须是对象")?;
            for key in [
                "local_file",
                "image_source",
                "image_error",
                "video_source",
                "video_complete",
                "video_bytes",
                "video_error",
            ] {
                object.remove(key);
            }
        }
    }
    for (post_index, post) in posts.iter().enumerate() {
        let tid = album_render::safe_stem(&post["tid"], &(post_index + 1).to_string());
        for (media_index, media) in post["media"].as_array().unwrap().iter().enumerate() {
            let kind = media
                .get("type")
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default();
            let stem = format!("{:05}_{tid}_{:02}", post_index + 1, media_index + 1);
            let job = Job {
                post_index,
                media_index,
                stem,
                post,
                media,
            };
            match kind.as_str() {
                "2" => {
                    for extension in ["jpg", "png", "gif", "webp"] {
                        targets.push(
                            PathBuf::from("images").join(format!("{}.{}", job.stem, extension)),
                        );
                    }
                    image_jobs.push(job);
                }
                "6" | "15" => {
                    targets.push(PathBuf::from("videos").join(format!("{}.mp4", job.stem)));
                    video_jobs.push(job);
                }
                _ => (),
            }
        }
    }
    let tree = publish::prepare(
        &options.output,
        &options.binding,
        options.policy,
        &targets,
        &options.inputs,
        |root| legacy_identity(root, &options.user, &options.binding.user_name),
    )?;
    let staging = tempfile::Builder::new()
        .prefix(".wx-album-")
        .tempdir_in(tree.root().parent().context("相册输出目录缺少父目录")?)?;
    fs::create_dir(staging.path().join("images"))?;
    fs::create_dir(staging.path().join("videos"))?;
    let image_guard = HostOutputGuard::new(&staging.path().join("images"))?;
    let video_guard = HostOutputGuard::new(&staging.path().join("videos"))?;
    let mut results = run_jobs(&image_jobs, options.image_workers.min(32), |job, engine| {
        image_job(
            job,
            tree.guard(Path::new("images"))?,
            &image_guard,
            options.no_remote,
            engine,
        )
    })?;
    eprintln!("图片 {} 项处理完成", image_jobs.len());
    if !options.no_videos {
        results.extend(run_jobs(
            &video_jobs,
            options.video_workers.min(16),
            |job, engine| {
                video_job(
                    job,
                    tree.guard(Path::new("videos"))?,
                    &video_guard,
                    cache,
                    options.no_remote,
                    engine,
                )
            },
        )?);
        eprintln!("视频 {} 项处理完成", video_jobs.len());
    }
    let video_total = video_jobs.len();
    drop(image_jobs);
    drop(video_jobs);
    let mut counts = [0usize; 8];
    let mut entries = Vec::new();
    for item in results {
        counts[item.status as usize] += 1;
        posts[item.post_index]["media"][item.media_index]
            .as_object_mut()
            .unwrap()
            .extend(item.fields);
        entries.extend(item.staged);
    }
    let rendered = album_render::render(&options.user, &posts)?;
    let output = std::path::absolute(&options.output)?;
    let mut warnings = options.warnings.clone();
    if let Some(cache) = cache {
        warnings.extend(cache.warnings.clone());
    }
    if tree.legacy_unverified() {
        warnings.push("已显式认领旧目录；旧文件与复用媒体的历史账号来源未经验证".into());
    }
    let [image_existing, image_remote, image_missing, video_existing, video_cache, video_remote, video_partial_cache, video_missing] =
        counts;
    let summary = json!({
        "engine":"rust", "user":options.user, "username":options.binding.user_name,
        "posts":posts.len(), "album_posts":rendered.album_posts,
        "first":posts.last().and_then(|p| p.get("time")), "last":posts.first().and_then(|p| p.get("time")),
        "image_ok":image_existing + image_remote, "image_existing":image_existing,
        "image_cache":0, "image_remote":image_remote, "image_missing":image_missing,
        "video_total":video_total, "video_ok":video_existing + video_cache + video_remote + video_partial_cache,
        "video_complete":video_existing + video_cache + video_remote,
        "video_existing":video_existing, "video_cache":video_cache, "video_remote":video_remote,
        "video_partial_cache":video_partial_cache, "video_missing":video_missing,
        "video_skipped":if options.no_videos { video_total } else { 0 },
        "output_dir":output, "timeline_json":output.join("timeline.json"), "html":output.join("timeline.html"),
        "legacy_unverified":tree.legacy_unverified(), "warnings":warnings,
    });
    for (name, bytes) in [
        ("timeline.json", serde_json::to_vec_pretty(&posts)?),
        ("timeline.html", rendered.html.into_bytes()),
        ("export_summary.json", serde_json::to_vec_pretty(&summary)?),
    ] {
        let staged = staging.path().join(name);
        fs::write(&staged, bytes)?;
        entries.push((PathBuf::from(name), staged));
    }
    tree.verify_all()?;
    tree.publish_all(&entries)?;
    // 目录守卫会阻止 Windows 删除暂存目录，先释放再由 TempDir 清理。
    drop(image_guard);
    drop(video_guard);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_keep_order_and_stop_taking_jobs_after_output_failure() {
        let post = json!({});
        let media = json!({});
        let jobs: Vec<_> = (0..20)
            .map(|index| Job {
                post_index: index,
                media_index: 0,
                stem: index.to_string(),
                post: &post,
                media: &media,
            })
            .collect();
        let results = run_jobs(&jobs, 3, |job, _| {
            Ok(result(job, Status::ImageMissing, json!({}), None))
        })
        .unwrap();
        assert_eq!(
            results.iter().map(|r| r.post_index).collect::<Vec<_>>(),
            (0..20).collect::<Vec<_>>()
        );
        let started = AtomicUsize::new(0);
        let failure = run_jobs(&jobs, 1, |_, _| {
            started.fetch_add(1, Ordering::SeqCst);
            bail!("synthetic output refusal");
        });
        assert!(failure.is_err());
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }
}
