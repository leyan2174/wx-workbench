use super::cache::{self, CacheIndex, CacheKeys, MediaRecovery, RecoveryOptions};
pub(crate) use super::download::Options as DownloadOptions;
use super::{download, publish};
use super::{parse_timeline, timestamp_filename, Comment, Content, Post, TimeZone};
use crate::attachment::local_files::HostOutputGuard;
use anyhow::{bail, ensure, Context, Result};
use chrono::Utc;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    pub timezone: TimeZone,
    /// 与旧 WECHAT_EXPORT_CONTACTS 一致：按数据库 user_name 精确筛选；空集合不过滤。
    pub contacts: BTreeSet<String>,
    /// 用于可重现导出；None 使用当前时间。
    pub export_time: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Timeline {
    pub user_name: String,
    pub display_name: String,
    pub export_time: String,
    pub total_posts: usize,
    pub posts: Vec<Post>,
}

#[derive(Clone, Debug, Default)]
pub struct ExportData {
    pub timelines: Vec<Timeline>,
    pub rows_seen: usize,
    pub filtered: usize,
    pub invalid: usize,
    /// 诊断只含行号/错误类别，不回显 XML、联系人名或媒体令牌。
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ExportReport {
    pub contacts: usize,
    pub posts: usize,
    pub files: Vec<PathBuf>,
    pub rows_seen: usize,
    pub filtered: usize,
    pub invalid: usize,
    pub warnings: Vec<String>,
    pub media_recovered: usize,
    pub media_missing: usize,
    pub media_failed: usize,
    /// 成功下载数是 media_recovered 的子集；失败下载数仅统计实际尝试。
    pub media_downloaded: usize,
    pub media_download_failed: usize,
    /// 显式认领过旧目录的联系人数量；历史账号来源仍未验证。
    pub legacy_unverified: usize,
}

/// 宿主固定来源；每个联系人再绑定精确 user_name，旧目录认领必须显式授权。
#[derive(Clone)]
pub(crate) struct TimelinePublication {
    pub flat_cache: bool,
    pub source_kind: String,
    pub source_id: String,
    pub policy: publish::ExistingPolicy,
    pub inputs: Vec<PathBuf>,
}

pub struct CacheRecovery<'a> {
    pub index: &'a CacheIndex,
    pub keys: &'a CacheKeys,
}

/// 兼容旧字符替换，同时封堵 Windows 保留名、尾点及父目录穿越。
pub fn safe_dirname(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = replaced.trim().trim_end_matches(['.', ' ']);
    let mut clean = if clean.is_empty() {
        "unknown".to_string()
    } else {
        clean.to_string()
    };
    let stem = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p).is_some_and(|s| {
                matches!(
                    s,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        clean.insert(0, '_');
    }
    clean
}

/// 只使用调用者提供的连接；None 等价于未提供联系人数据库。
pub fn load_contacts(conn: Option<&Connection>) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let Some(conn) = conn else {
        return Ok(map);
    };
    let mut statement = conn.prepare("SELECT username, remark, nick_name FROM contact")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let username = row.get::<_, Option<String>>(0)?.unwrap_or_default();
        let remark = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        let nickname = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let display = if !remark.is_empty() {
            &remark
        } else if !nickname.is_empty() {
            &nickname
        } else {
            &username
        };
        let display = safe_dirname(display);
        map.insert(username, display);
    }
    Ok(map)
}

/// 缺表/缺列返回错误，由 read_database 转为可见警告并继续（旧实现返回空映射）。
pub fn load_comments(conn: &Connection, timezone: TimeZone) -> Result<BTreeMap<i64, Vec<Comment>>> {
    let mut result = BTreeMap::<i64, Vec<Comment>>::new();
    let mut statement = conn.prepare("SELECT feed_id, create_time, type, from_username, from_nickname, to_username, to_nickname, content FROM SnsMessage_tmp3 WHERE COALESCE(del_status, 0) = 0 ORDER BY create_time")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let feed_id: i64 = row.get(0)?;
        let create_time: Option<i64> = row.get(1)?;
        let kind: Option<i64> = row.get(2)?;
        let name = match kind {
            Some(1) => "点赞".into(),
            Some(2) => "评论".into(),
            Some(n) => format!("未知({n})"),
            None => "未知(None)".into(),
        };
        result.entry(feed_id).or_default().push(Comment {
            create_time,
            create_time_str: timezone.display(create_time.unwrap_or(0))?,
            kind,
            type_name: name,
            from_username: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            from_nickname: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            to_username: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            to_nickname: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            content: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        });
    }
    Ok(result)
}

/// 内存连接可直接用于合成测试；只发出 SELECT，不读取配置或磁盘上的默认数据库。
pub fn read_database(
    sns: &Connection,
    contacts: Option<&Connection>,
    options: &ExportOptions,
) -> Result<ExportData> {
    let mut data = ExportData::default();
    let names = load_contacts(contacts).unwrap_or_else(|_| {
        data.warnings
            .push("contact table unavailable or invalid; using XML nickname/username".into());
        BTreeMap::new()
    });
    let comments = load_comments(sns, options.timezone).unwrap_or_else(|_| {
        data.warnings
            .push("SnsMessage_tmp3 unavailable or invalid; interactions omitted".into());
        BTreeMap::new()
    });
    let mut groups = BTreeMap::<String, Vec<Post>>::new();
    let mut nicknames = BTreeMap::<String, String>::new();
    let mut statement =
        sns.prepare("SELECT tid, user_name, content FROM SnsTimeLine WHERE content IS NOT NULL")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        data.rows_seen += 1;
        let username: Option<String> = row.get(1)?;
        if !options.contacts.is_empty()
            && !username
                .as_ref()
                .is_some_and(|u| options.contacts.contains(u))
        {
            data.filtered += 1;
            continue;
        }
        let parsed = match row.get_ref(2)? {
            ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                .map_err(anyhow::Error::from)
                .and_then(|s| parse_timeline(Content::Text(s), options.timezone)),
            ValueRef::Blob(bytes) => parse_timeline(Content::Blob(bytes), options.timezone),
            _ => parse_timeline(Content::Null, options.timezone),
        };
        let mut post = match parsed {
            Ok(Some(post)) => post,
            _ => {
                data.invalid += 1;
                data.warnings
                    .push(format!("SNS row {} could not be parsed", data.rows_seen));
                continue;
            }
        };
        let tid: i64 = row.get(0)?;
        post.tid = Some(tid);
        post.db_user_name = Some(username.clone().unwrap_or_default());
        post.comments = Some(comments.get(&tid).cloned().unwrap_or_default());
        let key = username
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        if !post.nickname.is_empty() {
            nicknames
                .entry(key.clone())
                .or_insert_with(|| post.nickname.clone());
        }
        groups.entry(key).or_default().push(post);
    }
    let now = options
        .export_time
        .unwrap_or_else(|| Utc::now().timestamp());
    let export_time = options.timezone.format(now, "%Y-%m-%d %H:%M:%S")?;
    let mut used_names = BTreeSet::new();
    for (username, mut posts) in groups {
        let base = safe_dirname(
            names
                .get(&username)
                .or_else(|| nicknames.get(&username))
                .unwrap_or(&username),
        );
        let mut display = base.clone();
        let mut counter = 1;
        // Windows 大小写不敏感，不能把同显示名的两个联系人写入同一目录。
        while !used_names.insert(display.to_lowercase()) {
            display = format!("{base}_{counter}");
            counter += 1;
        }
        posts.sort_by_key(|p| std::cmp::Reverse(p.create_time));
        data.timelines.push(Timeline {
            user_name: username,
            display_name: display,
            export_time: export_time.clone(),
            total_posts: posts.len(),
            posts,
        });
    }
    Ok(data)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn timeline_html(timeline: &Timeline, local_media: &BTreeMap<usize, Vec<MediaRecovery>>) -> String {
    // 媒体 URL 只作为文本引用展示，打开离线页面不会自动请求远端资源。
    let mut html = format!("<!DOCTYPE html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\"><title>{0} - 朋友圈</title><style>body{{font-family:system-ui;max-width:800px;margin:auto;padding:20px;color:#222;background:#fff}}article{{padding:16px 0;border-bottom:1px solid #ddd}}h1{{overflow-wrap:anywhere}}p,li{{white-space:pre-wrap;overflow-wrap:anywhere}}time{{color:#666}}</style><body><h1>{0} 的朋友圈</h1><p>共 {1} 条动态</p>", escape(&timeline.display_name), timeline.total_posts);
    if !local_media.is_empty() {
        html = html.replace(
            "default-src 'none'; style-src 'unsafe-inline'",
            "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; media-src 'self'",
        );
    }
    for (post_index, post) in timeline.posts.iter().enumerate() {
        html.push_str(&format!(
            "<article><time>{}</time> <span>{}</span>{}<p>{}</p>",
            escape(&post.create_time_str),
            escape(&post.content_type_name),
            if post.is_private {
                " <span>仅自己可见</span>"
            } else {
                ""
            },
            escape(&post.content_desc)
        ));
        if let Some(loc) = &post.location {
            html.push_str(&format!("<p>{}</p>", escape(&loc.poi_name)));
        }
        for item in local_media.get(&post_index).into_iter().flatten() {
            let Some(path) = item
                .reference
                .as_ref()
                .and_then(|value| value["local_file"].as_str())
            else {
                continue;
            };
            // 引用仅来自本次恢复/下载结果；URL/原始 XML 不进入 src 属性。
            let downloaded_format = item
                .reference
                .as_ref()
                .and_then(|value| value["download_format"].as_str());
            if matches!(downloaded_format, Some("jpg" | "png" | "gif" | "webp")) {
                html.push_str(&format!(
                    "<p><img src=\"{}\" alt=\"本地图片\" style=\"max-width:100%;height:auto\"></p>",
                    escape(path)
                ));
            } else if matches!(downloaded_format, Some("mp4" | "mov")) {
                html.push_str(&format!("<p><video src=\"{}\" controls preload=\"metadata\" style=\"max-width:100%\"></video></p>", escape(path)));
            } else if downloaded_format == Some("bin") {
                html.push_str(&format!(
                    "<p><a href=\"{}\" download>下载媒体文件</a></p>",
                    escape(path)
                ));
            } else if item
                .reference
                .as_ref()
                .is_some_and(|value| value["image_source"] == "cache")
            {
                html.push_str(&format!("<p><img src=\"{}\" alt=\"本地缓存图片\" style=\"max-width:100%;height:auto\"></p>", escape(path)));
            } else if item
                .reference
                .as_ref()
                .is_some_and(|value| value["video_source"] == "cache")
            {
                html.push_str(&format!("<p><video src=\"{}\" controls preload=\"metadata\" style=\"max-width:100%\"></video></p>", escape(path)));
            }
        }
        for media in &post.media {
            for key in ["url", "thumb_url"] {
                if let Some(url) = media.get(key).filter(|v| !v.is_empty()) {
                    html.push_str(&format!("<p>{}: {}</p>", key, escape(url)));
                }
            }
        }
        if let Some(comments) = &post.comments {
            html.push_str("<ul>");
            for c in comments {
                html.push_str(&format!(
                    "<li>{} {}{}: {}</li>",
                    escape(&c.type_name),
                    escape(&c.from_nickname),
                    if c.to_nickname.is_empty() {
                        String::new()
                    } else {
                        format!(" 回复 {}", escape(&c.to_nickname))
                    },
                    escape(&c.content)
                ));
            }
            html.push_str("</ul>");
        }
        html.push_str("</article>");
    }
    html.push_str("</body></html>");
    html
}

/// 为避免不同账号或重跑覆盖，要求每个目标 SNS 目录尚不存在。
/// 建议 preview 为每次导出分配全新的账号专属输出目录。
pub(super) fn write_export_with_media(
    data: &ExportData,
    output: &Path,
    timezone: TimeZone,
    cache: Option<&CacheRecovery<'_>>,
    download_options: Option<&DownloadOptions>,
) -> Result<ExportReport> {
    write_export_with_publication(data, output, timezone, cache, download_options, None)
}

fn post_names(timeline: &Timeline, timezone: TimeZone) -> Result<Vec<(usize, String)>> {
    let mut ascending: Vec<_> = timeline.posts.iter().enumerate().collect();
    ascending.sort_by_key(|(_, post)| post.create_time);
    let mut used = BTreeSet::new();
    ascending
        .into_iter()
        .map(|(index, post)| {
            let stem = timestamp_filename(post.create_time, timezone)?;
            let mut name = stem.clone();
            let mut counter = 1;
            while !used.insert(name.clone()) {
                name = format!("{}{:03}", &stem[..stem.len() - 3], counter);
                counter += 1;
            }
            Ok((index, name))
        })
        .collect()
}

fn publication_targets(
    timeline: &Timeline,
    names: &[(usize, String)],
    cache: bool,
    download: bool,
    flat_cache: bool,
) -> Vec<PathBuf> {
    let mut targets = vec!["timeline.json".into(), "timeline.html".into()];
    if cache || download {
        targets.push("_media_recovery.json".into());
    }
    for (post_index, stem) in names {
        targets.push(format!("{stem}.json").into());
        for media_index in 0..timeline.posts[*post_index].media.len() {
            let name = format!("{stem}_{media_index}");
            if download {
                for extension in ["jpg", "png", "gif", "webp", "mp4", "mov", "bin"] {
                    targets.push(format!("{name}.{extension}").into());
                }
            }
            if cache {
                for extension in ["jpg", "png", "gif", "webp"] {
                    targets.push(
                        PathBuf::from(if flat_cache { "" } else { "images" })
                            .join(format!("{name}.{extension}")),
                    );
                }
                targets.push(
                    PathBuf::from(if flat_cache { "" } else { "videos" })
                        .join(format!("{name}.mp4")),
                );
            }
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn legacy_post_identity(value: &serde_json::Value, username: &str) -> Result<()> {
    ensure!(value.is_object(), "旧 SNS 帖子必须是对象");
    if let Some(raw) = value.get("db_user_name") {
        let raw = raw.as_str().context("旧 SNS 帖子作者字段无效")?;
        ensure!(
            (if raw.is_empty() { "unknown" } else { raw }) == username,
            "旧 SNS 帖子包含其他联系人，拒绝认领"
        );
    }
    Ok(())
}

fn legacy_timeline_identity(root: &Path, username: &str) -> Result<()> {
    if let Some(value) = publish::read_legacy_json(&root.join("timeline.json"), 256 * 1024 * 1024)?
    {
        ensure!(
            value.get("user_name").and_then(serde_json::Value::as_str) == Some(username),
            "旧 SNS 汇总作者不匹配；相册数组不能作为时间线认领"
        );
        for post in value["posts"]
            .as_array()
            .context("旧 SNS 汇总缺少帖子列表")?
        {
            legacy_post_identity(post, username)?;
        }
    }
    // 只核验旧时间戳帖子文件，不递归或解读无关文件；历史账号来源仍标为未验证。
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| {
                    stem.len() >= 17 && stem.bytes().all(|byte| byte.is_ascii_digit())
                })
        {
            let value = publish::read_legacy_json(&path, 256 * 1024 * 1024)?
                .context("旧 SNS 帖子在校验时消失")?;
            legacy_post_identity(&value, username)?;
        }
    }
    Ok(())
}

fn write_export_with_publication(
    data: &ExportData,
    output: &Path,
    timezone: TimeZone,
    cache: Option<&CacheRecovery<'_>>,
    download_options: Option<&DownloadOptions>,
    publication: Option<&TimelinePublication>,
) -> Result<ExportReport> {
    let mut report = ExportReport {
        rows_seen: data.rows_seen,
        filtered: data.filtered,
        invalid: data.invalid,
        warnings: data.warnings.clone(),
        ..ExportReport::default()
    };
    if let Some(cache) = cache {
        report.warnings.extend(cache.index.warnings.clone());
    }
    if publication.is_some() && data.timelines.is_empty() {
        return Ok(report);
    }
    let output = if publication.is_some() {
        std::path::absolute(output)?
    } else {
        fs::create_dir_all(output)?;
        fs::canonicalize(output)?
    };
    let mut planned = BTreeSet::new();
    let mut names = Vec::new();
    for timeline in &data.timelines {
        if safe_dirname(&timeline.display_name) != timeline.display_name
            || !planned.insert(timeline.display_name.to_lowercase())
        {
            bail!("SNS display name is unsafe or duplicated");
        }
        let parent = output.join(&timeline.display_name);
        if publication.is_none()
            && parent.exists()
            && !fs::canonicalize(&parent)?.starts_with(&output)
        {
            bail!("SNS output escapes destination");
        }
        if publication.is_none() && parent.join("SNS").symlink_metadata().is_ok() {
            bail!("SNS destination already exists; use a fresh output directory");
        }
        names.push(post_names(timeline, timezone)?);
    }
    // 所有联系人先预检并持有绑定锁，再开始本轮内容生成；晚冲突不会提交前面帖子的内容。
    let mut trees = Vec::new();
    if let Some(publication) = publication {
        for (timeline, names) in data.timelines.iter().zip(&names) {
            let root = output.join(&timeline.display_name).join("SNS");
            let tree = publish::prepare(
                &root,
                &publish::Binding {
                    version: 1,
                    tree_kind: "timeline".into(),
                    source_kind: publication.source_kind.clone(),
                    source_id: publication.source_id.clone(),
                    user_name: timeline.user_name.clone(),
                },
                publication.policy,
                &publication_targets(
                    timeline,
                    names,
                    cache.is_some(),
                    download_options.is_some(),
                    publication.flat_cache,
                ),
                &publication.inputs,
                |root| legacy_timeline_identity(root, &timeline.user_name),
            )?;
            if tree.legacy_unverified() {
                report.legacy_unverified += 1;
                report
                    .warnings
                    .push("已显式认领旧 SNS 目录；历史账号与媒体来源未经验证".into());
            }
            trees.push(tree);
        }
    }
    for (timeline_index, (timeline, names)) in data.timelines.iter().zip(&names).enumerate() {
        let dir = output.join(&timeline.display_name).join("SNS");
        fs::create_dir_all(dir.parent().unwrap())?;
        // 完整联系人目录写好后才发布；中断时临时目录自动清理，便于重试。
        let staging = tempfile::Builder::new()
            .prefix(".wx-sns-")
            .tempdir_in(dir.parent().unwrap())?;
        let download_guard = download_options
            .map(|_| HostOutputGuard::new(staging.path()))
            .transpose()?;
        let mut files = Vec::new();
        let mut timeline_json = serde_json::to_value(timeline)?;
        let mut local_media = BTreeMap::new();
        let mut recovery_reports = Vec::new();
        for (post_index, name) in names {
            let post_index = *post_index;
            let post = &timeline.posts[post_index];
            let filename = format!("{name}.json");
            let path = staging.path().join(&filename);
            if cache.is_some() || download_options.is_some() {
                let mut value = serde_json::to_value(post)?;
                let (mut media, mut warnings) = if let Some(cache) = cache {
                    for (index, id) in post.cache_media_ids.iter().enumerate() {
                        if !id.is_empty() {
                            value["media"][index]["id"] = id.clone().into();
                        }
                    }
                    let mut recovery = cache::recover_post_media(
                        cache.index,
                        &value,
                        staging.path(),
                        name,
                        cache.keys,
                        RecoveryOptions::default(),
                    )?;
                    if publication.is_some_and(|policy| policy.flat_cache) {
                        // 旧时间线的媒体与帖子同级；仅移动本轮私有暂存目录中恢复的文件。
                        for item in &mut recovery.media {
                            if let Some(reference) = &mut item.reference {
                                let local = reference["local_file"]
                                    .as_str()
                                    .context("缓存媒体缺少路径")?;
                                let filename = Path::new(local)
                                    .file_name()
                                    .context("缓存媒体路径无文件名")?
                                    .to_owned();
                                fs::rename(
                                    staging.path().join(local),
                                    staging.path().join(&filename),
                                )?;
                                reference["local_file"] =
                                    filename.to_string_lossy().into_owned().into();
                            }
                        }
                    }
                    cache::apply_media_references(&mut value, &recovery)?;
                    (recovery.media, recovery.warnings)
                } else {
                    (
                        post.media
                            .iter()
                            .enumerate()
                            .map(|(media_index, _)| MediaRecovery {
                                media_index,
                                status: "missing".into(),
                                reference: None,
                                match_method: None,
                                bytes: 0,
                            })
                            .collect(),
                        Vec::new(),
                    )
                };
                if let (Some(options), Some(guard)) = (download_options, download_guard.as_ref()) {
                    for item in &mut media {
                        // 只跳过本轮真实恢复成功项，不把失败/不支持类型误当作缓存命中。
                        if item.reference.is_some() {
                            continue;
                        }
                        let source = &post.media[item.media_index];
                        let Some(url) = source
                            .get("url")
                            .filter(|s| !s.is_empty())
                            .or_else(|| source.get("thumb_url").filter(|s| !s.is_empty()))
                        else {
                            continue;
                        };
                        let destination =
                            staging.path().join(format!("{name}_{}", item.media_index));
                        match download::download(url, &destination, guard, options) {
                            Ok(outcome) => {
                                let reference = serde_json::json!({
                                    "local_file": outcome.actual_filename,
                                    "media_source": "download",
                                    "download_format": outcome.format.extension(),
                                });
                                // 原媒体字段和顺序保持不变；同一结果用于单帖、汇总及 HTML。
                                value["media"][item.media_index]
                                    .as_object_mut()
                                    .context("invalid SNS media object")?
                                    .extend(reference.as_object().unwrap().clone());
                                item.reference = Some(reference);
                                item.status = "downloaded".into();
                                item.match_method = Some("http_download".into());
                                item.bytes = outcome.bytes;
                                report.media_downloaded += 1;
                            }
                            Err(_) => {
                                // 不串联 URL 或文件系统错误，诊断只含本轮媒体序号。
                                item.status = "download_failed".into();
                                report.media_download_failed += 1;
                                warnings.push(format!(
                                    "media {}: SNS download failed",
                                    item.media_index
                                ));
                            }
                        }
                    }
                }
                for item in &media {
                    match item.status.as_str() {
                        "recovered" | "downloaded" => report.media_recovered += 1,
                        "failed" | "invalid_metadata" | "download_failed" => {
                            report.media_failed += 1
                        }
                        _ => report.media_missing += 1,
                    }
                    if let Some(file) = item
                        .reference
                        .as_ref()
                        .and_then(|v| v["local_file"].as_str())
                    {
                        files.push(dir.join(file));
                    }
                }
                report.warnings.extend(
                    warnings
                        .iter()
                        .map(|warning| format!("{filename}: {warning}")),
                );
                recovery_reports.push(serde_json::json!({"post_file":filename,
                    "recovery":{"media":media,"warnings":warnings}}));
                local_media.insert(post_index, media);
                timeline_json["posts"][post_index] = value.clone();
                fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
            } else {
                fs::write(&path, serde_json::to_vec_pretty(post)?)?;
            }
            files.push(dir.join(filename));
        }
        let summary = staging.path().join("timeline.json");
        fs::write(
            &summary,
            if cache.is_some() || download_options.is_some() {
                serde_json::to_vec_pretty(&timeline_json)?
            } else {
                serde_json::to_vec_pretty(timeline)?
            },
        )?;
        files.push(dir.join("timeline.json"));
        let html = staging.path().join("timeline.html");
        fs::write(&html, timeline_html(timeline, &local_media))?;
        files.push(dir.join("timeline.html"));
        if cache.is_some() || download_options.is_some() {
            fs::write(
                staging.path().join("_media_recovery.json"),
                serde_json::to_vec_pretty(&recovery_reports)?,
            )?;
            files.push(dir.join("_media_recovery.json"));
        }
        // Windows 守卫会禁止目录重命名；发布前核验并释放，沿用 fresh 目录策略。
        if let Some(guard) = &download_guard {
            guard.verify()?;
        }
        drop(download_guard);
        if let Some(tree) = trees.get(timeline_index) {
            let mut entries = files
                .iter()
                .map(|file| {
                    let relative = file.strip_prefix(&dir)?.to_path_buf();
                    Ok((relative.clone(), staging.path().join(relative)))
                })
                .collect::<Result<Vec<_>>>()?;
            // 媒体先于单帖，之后汇总与 HTML，恢复报告最后；不复用旧同名媒体。
            entries.sort_by_key(|(path, _)| match path.to_str() {
                Some("timeline.json") => 2,
                Some("timeline.html") => 3,
                Some("_media_recovery.json") => 4,
                _ if path.extension().is_some_and(|ext| ext == "json") => 1,
                _ => 0,
            });
            tree.publish_all(&entries)?;
        } else {
            fs::rename(staging.path(), &dir).context("publish SNS contact directory")?;
        }
        report.files.extend(files);
        report.posts += timeline.posts.len();
        report.contacts += 1;
    }
    Ok(report)
}

/// preview 主入口：数据库只读打开，返回实际写入文件及未解析/缺表诊断。
/// Some(download_options) 表示调用者已显式授权联网；None 完全离线。
/// 不读取环境或账号；异步宿主必须使用阻塞线程。仍要求目标 SNS 目录不存在。
pub fn export_database_with_media(
    sns_path: &Path,
    contacts_path: Option<&Path>,
    output: &Path,
    options: &ExportOptions,
    cache: Option<&CacheRecovery<'_>>,
    download_options: Option<&DownloadOptions>,
) -> Result<ExportReport> {
    export_database_selected(
        sns_path,
        contacts_path,
        output,
        options,
        cache,
        download_options,
        None,
    )
}

/// 同来源绑定目录更新；不合并旧汇总、不删除旧无关文件，内容发布允许部分提交。
pub(crate) fn export_database_with_publication(
    sns_path: &Path,
    contacts_path: Option<&Path>,
    output: &Path,
    options: &ExportOptions,
    cache: Option<&CacheRecovery<'_>>,
    download_options: Option<&DownloadOptions>,
    publication: &TimelinePublication,
) -> Result<ExportReport> {
    let sns_path = fs::canonicalize(sns_path)?;
    let contacts_path = contacts_path.map(fs::canonicalize).transpose()?;
    let mut publication = publication.clone();
    publication.inputs.push(sns_path.clone());
    publication.inputs.extend(contacts_path.iter().cloned());
    export_database_selected(
        &sns_path,
        contacts_path.as_deref(),
        output,
        options,
        cache,
        download_options,
        Some(&publication),
    )
}

fn export_database_selected(
    sns_path: &Path,
    contacts_path: Option<&Path>,
    output: &Path,
    options: &ExportOptions,
    cache: Option<&CacheRecovery<'_>>,
    download_options: Option<&DownloadOptions>,
    publication: Option<&TimelinePublication>,
) -> Result<ExportReport> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let sns =
        Connection::open_with_flags(sns_path, flags).context("open SNS database read-only")?;
    let contacts = contacts_path
        .map(|p| Connection::open_with_flags(p, flags))
        .transpose()
        .context("open contact database read-only")?;
    let snapshot = sns.unchecked_transaction()?;
    let data = read_database(&snapshot, contacts.as_ref(), options)?;
    match publication {
        Some(publication) => write_export_with_publication(
            &data,
            output,
            options.timezone,
            cache,
            download_options,
            Some(publication),
        ),
        None => write_export_with_media(&data, output, options.timezone, cache, download_options),
    }
}

#[cfg(all(test, windows))]
#[path = "export_download_tests.rs"]
mod download_tests;

#[cfg(all(test, windows))]
#[path = "export_update_tests.rs"]
mod update_tests;
