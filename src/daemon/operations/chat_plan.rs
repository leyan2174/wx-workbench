//! 显式离线计划 CSV 命令；不接触配置、IPC、账号发现或聊天正文导出。
use crate::toolkit::chat_plan as plan;
use anyhow::{ensure, Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum Mode {
    Estimate,
    Scan,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 明确指定已解密缓存根目录；不读取配置或发现账号
    #[arg(long)]
    pub decrypted_dir: PathBuf,
    /// 相对解密根目录的消息数据库路径，可重复；不指定时保留缺库状态
    #[arg(long = "message-db")]
    pub message_dbs: Vec<PathBuf>,
    /// 相对解密根目录的 message_resource 数据库路径
    #[arg(long)]
    pub resource_db: Option<PathBuf>,
    /// 相对解密根目录的语音数据库路径，可重复
    #[arg(long = "media-db")]
    pub media_dbs: Vec<PathBuf>,
    /// 明确的 username，可重复；提供元数据清单时作为精确过滤
    #[arg(long = "user", alias = "users", required_unless_present = "chats_json")]
    pub users: Vec<String>,
    /// 聊天元数据 JSON 数组：username、index、chat_name/display_name、chat_type/kind
    #[arg(long)]
    pub chats_json: Option<PathBuf>,
    /// 精确排除 username，可重复
    #[arg(long = "exclude-user")]
    pub exclude_users: Vec<String>,
    /// 统计模式；scan 另需显式源目录或媒体目录
    #[arg(long, value_enum, default_value = "estimate")]
    pub size_mode: Mode,
    /// 账号源目录，扫描其 msg 子目录；与 media-dir 二选一
    #[arg(long, conflicts_with = "media_dir")]
    pub source_dir: Option<PathBuf>,
    /// 直接指定包含 attach/file/video 的媒体目录
    #[arg(long, conflicts_with = "source_dir")]
    pub media_dir: Option<PathBuf>,
    /// 扫描线程数，结果仍按清单顺序输出
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=6))]
    pub threads: u8,
    /// 含端点的起始本地时间：日期、日期时间或 Unix 秒
    #[arg(long, allow_hyphen_values = true)]
    pub start: Option<String>,
    /// 含端点的结束时间；仅日期表示当天零点
    #[arg(long, allow_hyphen_values = true)]
    pub end: Option<String>,
    /// 新 CSV 文件；父目录须存在，禁止覆盖或写入源目录
    #[arg(short, long, alias = "write-plan-csv")]
    pub output: PathBuf,
}

#[derive(Deserialize)]
struct ChatMetadata {
    username: String,
    index: Option<usize>,
    #[serde(default, alias = "display_name")]
    chat_name: Option<String>,
    #[serde(default, alias = "kind")]
    chat_type: Option<String>,
}

fn timestamp(raw: &str) -> Result<i64> {
    let raw = raw.trim();
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    let date = date.or_else(|| {
        ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"]
            .into_iter()
            .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
    });
    if let Some(date) = date {
        return Local
            .from_local_datetime(&date)
            .single()
            .map(|d| d.timestamp())
            .context("本地时间不存在或有歧义，请使用 Unix 秒");
    }
    let value: i64 = raw.parse().context("无法解析时间")?;
    ensure!(
        Local.timestamp_opt(value, 0).single().is_some(),
        "时间超出支持范围"
    );
    Ok(value)
}

fn selected_chats(args: &Args) -> Result<Vec<plan::PlanChat>> {
    let mut metadata: Vec<ChatMetadata> = if let Some(path) = &args.chats_json {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 * 1024 * 1024, "聊天清单超过 16 MiB 上限");
        serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
            .context("聊天清单必须是 JSON 数组")?
    } else {
        ensure!(
            !args.users.is_empty(),
            "必须明确提供 --user 或 --chats-json"
        );
        args.users
            .iter()
            .map(|u| ChatMetadata {
                username: u.clone(),
                index: None,
                chat_name: None,
                chat_type: None,
            })
            .collect()
    };
    let mut known = BTreeSet::new();
    for row in &metadata {
        ensure!(
            !row.username.is_empty() && known.insert(row.username.clone()),
            "聊天清单 username 不能为空或重复"
        );
    }
    ensure!(
        args.users.iter().all(|u| known.contains(u)),
        "用户过滤包含清单中不存在的 username"
    );
    ensure!(
        args.exclude_users.iter().all(|u| !u.is_empty()),
        "排除的 username 不能为空"
    );
    let users: BTreeSet<_> = args.users.iter().collect();
    ensure!(users.len() == args.users.len(), "--user 不能重复");
    let excluded: BTreeSet<_> = args.exclude_users.iter().collect();
    // 先编号再过滤，保留原始清单 index，不按显示名合并同名联系人。
    let rows: Vec<_> = metadata
        .drain(..)
        .enumerate()
        .filter_map(|(i, row)| {
            if (!users.is_empty() && !users.contains(&row.username))
                || excluded.contains(&row.username)
            {
                return None;
            }
            Some(plan::PlanChat {
                index: row.index.unwrap_or(i + 1),
                chat_name: row.chat_name.unwrap_or_else(|| row.username.clone()),
                chat_type: row.chat_type.unwrap_or_else(|| {
                    if row.username.ends_with("@chatroom") {
                        "group".into()
                    } else {
                        "single".into()
                    }
                }),
                username: row.username,
            })
        })
        .collect();
    ensure!(!rows.is_empty(), "用户过滤后没有聊天；未写入输出");
    Ok(rows)
}

fn output_path(raw: &Path) -> Result<PathBuf> {
    ensure!(!raw.as_os_str().is_empty(), "输出路径不能为空");
    ensure!(
        !raw.components().any(|p| matches!(p, Component::ParentDir)),
        "输出路径不能包含 .."
    );
    for part in raw.components() {
        #[cfg(windows)]
        if let Component::Prefix(prefix) = part {
            use std::path::Prefix;
            ensure!(
                raw.is_absolute()
                    && matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
                "输出只支持本机绝对磁盘路径或普通相对路径"
            );
        }
        if let Component::Normal(name) = part {
            let name = name.to_string_lossy();
            ensure!(
                !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c))
                    && !name.ends_with([' ', '.']),
                "非法输出路径组件"
            );
        }
    }
    let path = std::path::absolute(raw)?;
    ensure!(path.file_name().is_some(), "输出路径必须包含文件名");
    Ok(path)
}

fn pin_output_parent(parent: &Path) -> Result<Vec<fs::File>> {
    let mut pins = Vec::new();
    let mut current = PathBuf::new();
    for part in parent.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        let mut options = fs::OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .access_mode(0x80)
                .share_mode(3)
                .custom_flags(0x02200000);
        }
        #[cfg(not(windows))]
        {
            options.read(true);
        }
        let file = options.open(&current).context("输出父目录须存在且可访问")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "输出父路径不是普通目录"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "输出父路径不能经过重解析点"
            );
        }
        pins.push(file);
    }
    Ok(pins)
}

fn outside_sources(output: &Path, args: &Args) -> Result<()> {
    let output = output.to_string_lossy().replace('/', "\\").to_lowercase();
    for source in [
        Some(&args.decrypted_dir),
        args.source_dir.as_ref(),
        args.media_dir.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        ensure!(source.is_absolute(), "源目录必须明确指定绝对路径");
        let source = source
            .canonicalize()
            .unwrap_or(std::path::absolute(source)?);
        let source = source
            .to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        let output = output.trim_start_matches("\\\\?\\");
        ensure!(
            output != source && !output.starts_with(&format!("{source}\\")),
            "输出必须位于解密缓存和扫描源目录之外"
        );
    }
    Ok(())
}

fn publish(
    path: &Path,
    bytes: &[u8],
    replacement: Option<&crate::attachment::local_files::HostOutputGuard>,
) -> Result<()> {
    let parent = path.parent().context("输出路径缺少父目录")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".wx-chat-plan-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if let Some(guard) = replacement {
        guard.verify_replaceable_file(path)?;
        temporary
            .persist(path)
            .map_err(|e| e.error)
            .context("计划 CSV 更新失败，文件可能正在被其他程序使用")?;
    } else {
        // 独立离线入口仍为新建模式；目标竞争出现时失败，不覆盖旧文件。
        temporary
            .persist_noclobber(path)
            .map_err(|e| e.error)
            .context("计划 CSV 发布失败，目标可能已存在")?;
    }
    Ok(())
}

pub fn cmd(args: Args) -> Result<()> {
    let count = execute(&args)?;
    println!(
        "已写入计划 CSV：{}（{} 个聊天）",
        args.output.display(),
        count
    );
    Ok(())
}

fn execute(args: &Args) -> Result<usize> {
    execute_for(args, None, None, false)
}

/// 统一导出入口直接传入已选会话，不为调用计划核心创建中间清单文件。
pub(super) fn execute_for(
    args: &Args,
    chats: Option<Vec<plan::PlanChat>>,
    default_export: Option<&str>,
    replace_existing: bool,
) -> Result<usize> {
    ensure!(
        (1..=6).contains(&args.threads),
        "threads 必须在 1..=6 范围内"
    );
    ensure!(
        args.source_dir.is_none() || args.media_dir.is_none(),
        "source-dir 与 media-dir 只能指定一个"
    );
    match args.size_mode {
        Mode::Scan => ensure!(
            args.source_dir.is_some() || args.media_dir.is_some(),
            "scan 必须明确指定 source-dir 或 media-dir"
        ),
        Mode::Estimate => ensure!(
            args.source_dir.is_none() && args.media_dir.is_none(),
            "estimate 不使用扫描源目录，请移除参数或选择 scan"
        ),
    }
    let range = plan::TimeRange {
        start: args.start.as_deref().map(timestamp).transpose()?,
        end: args.end.as_deref().map(timestamp).transpose()?,
    };
    ensure!(
        !matches!((range.start, range.end), (Some(a), Some(b)) if a > b),
        "起始时间不能晚于结束时间"
    );
    let chats = match chats {
        Some(chats) => chats,
        None => selected_chats(args)?,
    };
    let output = output_path(&args.output)?;
    let replacement = if replace_existing {
        Some(crate::attachment::local_files::HostOutputGuard::new(
            output.parent().context("输出路径缺少父目录")?,
        )?)
    } else {
        None
    };
    if replace_existing {
        fs::create_dir_all(output.parent().context("输出路径缺少父目录")?)?;
    }
    let _pins = pin_output_parent(output.parent().context("输出路径缺少父目录")?)?;
    // 用规范父路径比较，避免 Windows 8.3 短路径别名绕过源目录隔离。
    let output = output
        .parent()
        .context("输出路径缺少父目录")?
        .canonicalize()?
        .join(output.file_name().context("输出缺少文件名")?);
    outside_sources(&output, args)?;
    if let Some(guard) = &replacement {
        guard.verify_replaceable_file(&output)?;
    } else {
        ensure!(
            matches!(fs::symlink_metadata(&output), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
            "输出已存在或不可访问，禁止覆盖"
        );
    }
    let databases = plan::PlanDatabases {
        message: args.message_dbs.clone(),
        resource: args.resource_db.clone(),
        media: args.media_dbs.clone(),
    };
    let mut rows = match args.size_mode {
        Mode::Estimate => plan::collect_plan(
            &args.decrypted_dir,
            &databases,
            &chats,
            range,
            plan::SizeMode::Estimate,
        )?,
        Mode::Scan => plan::collect_plan_with_scan(
            &args.decrypted_dir,
            &databases,
            &chats,
            range,
            &plan::ScanOptions {
                source_dir: args.source_dir.clone(),
                media_dir: args.media_dir.clone(),
                workers: args.threads as usize,
            },
        )?,
    };
    if let Some(flag) = default_export {
        for row in &mut rows {
            row.export = flag.to_owned();
        }
    }
    publish(
        &output,
        &plan::render_plan_csv(&rows)?,
        replacement.as_ref(),
    )?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Command {
        #[command(flatten)]
        args: Args,
    }

    fn args(root: &Path) -> Args {
        Args {
            decrypted_dir: root.join("cache"),
            message_dbs: vec![],
            resource_db: None,
            media_dbs: vec![],
            users: vec!["alpha".into()],
            chats_json: None,
            exclude_users: vec![],
            size_mode: Mode::Estimate,
            source_dir: None,
            media_dir: None,
            threads: 1,
            start: None,
            end: None,
            output: root.join("plan.csv"),
        }
    }

    #[test]
    fn argument_contract() {
        use clap::CommandFactory;
        Command::command().debug_assert();
        let base = [
            "plan",
            "--decrypted-dir",
            "C:\\synthetic",
            "--output",
            "C:\\plan.csv",
            "--user",
            "alpha",
        ];
        assert!(Command::try_parse_from(base).is_ok());
        for n in ["0", "7", "999"] {
            assert!(Command::try_parse_from(base.into_iter().chain(["--threads", n])).is_err());
        }
        assert!(Command::try_parse_from(base.into_iter().chain([
            "--source-dir",
            "C:\\source",
            "--media-dir",
            "C:\\media"
        ]))
        .is_err());
        assert!(Command::try_parse_from(&base[..5]).is_err());
    }

    fn fixture(root: &Path) -> serde_json::Value {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/chat-plan-golden.json"
        ))
        .unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        for (name, sql) in value["sql"].as_object().unwrap() {
            rusqlite::Connection::open(root.join("cache").join(name))
                .unwrap()
                .execute_batch(sql.as_str().unwrap())
                .unwrap();
        }
        value
    }

    #[test]
    fn command_estimate_keeps_all_fields_nulls_and_precision() {
        let temp = tempfile::tempdir().unwrap();
        fixture(temp.path());
        let mut args = args(temp.path());
        args.message_dbs = vec!["messages.db".into()];
        args.resource_db = Some("resource.db".into());
        args.media_dbs = vec!["media.db".into()];
        args.start = Some("100".into());
        args.end = Some("102".into());
        let manifest = temp.path().join("chats.json");
        fs::write(&manifest, r#"[{"index":9007199254740993,"username":"alpha","display_name":"中文, \"名字\"","kind":"single"},{"username":"absent","display_name":"中文, \"名字\""}]"#).unwrap();
        args.chats_json = Some(manifest);
        let output = args.output.clone();
        cmd(args).unwrap();
        let bytes = fs::read(&output).unwrap();
        assert_eq!(&bytes[..3], &[0xef, 0xbb, 0xbf]);
        let mut reader = csv::Reader::from_reader(&bytes[3..]);
        assert_eq!(
            reader.headers().unwrap().iter().collect::<Vec<_>>(),
            plan::PLAN_CSV_FIELDS
        );
        let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0][1], "9007199254740993");
        assert_eq!(&rows[0][3], "中文, \"名字\"");
        assert_eq!(&rows[0][5], "3");
        assert_eq!(&rows[0][8], "47");
        assert_eq!(&rows[0][9], "");
        assert_eq!(&rows[0][10], "57");
        assert_eq!(&rows[0][11], "ok");
    }

    #[test]
    fn command_scan_and_filters_preserve_order_and_missing_status() {
        let temp = tempfile::tempdir().unwrap();
        fixture(temp.path());
        let mut args = args(temp.path());
        args.users = vec!["alpha".into(), "absent".into(), "beta".into()];
        args.exclude_users = vec!["beta".into()];
        args.size_mode = Mode::Scan;
        args.threads = 6;
        let source = temp.path().join("source");
        let attach = source
            .join("msg/attach")
            .join(format!("{:x}", md5::compute("alpha")));
        fs::create_dir_all(&attach).unwrap();
        fs::write(attach.join("synthetic.bin"), b"abc").unwrap();
        args.source_dir = Some(source);
        assert_eq!(execute(&args).unwrap(), 2);
        let bytes = fs::read(&args.output).unwrap();
        let records: Vec<_> = csv::Reader::from_reader(&bytes[3..])
            .records()
            .map(Result::unwrap)
            .collect();
        assert_eq!(&records[0][2], "alpha");
        assert_eq!(&records[0][9], "3");
        assert_eq!(&records[1][2], "absent");
        assert_eq!(&records[1][9], "0");
        assert!(records[0][11].contains("message_db_missing"));
        assert_eq!(fs::read(attach.join("synthetic.bin")).unwrap(), b"abc");
    }

    #[test]
    fn publication_never_overwrites_and_cleans_temporary() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("plan.csv");
        publish(&output, b"original", None).unwrap();
        assert!(publish(&output, b"replacement", None).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"original");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn competing_publishers_have_exactly_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("plan.csv");
        let barrier = std::sync::Barrier::new(2);
        let successes = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                publish(&output, b"first", None).is_ok()
            });
            let b = scope.spawn(|| {
                barrier.wait();
                publish(&output, b"second", None).is_ok()
            });
            usize::from(a.join().unwrap()) + usize::from(b.join().unwrap())
        });
        assert_eq!(successes, 1);
        let bytes = fs::read(&output).unwrap();
        assert!(bytes == b"first" || bytes == b"second");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_inputs_leave_no_output() {
        let temp = tempfile::tempdir().unwrap();
        fixture(temp.path());
        let mut a = args(temp.path());
        a.output = a.decrypted_dir.join("out.csv");
        assert!(execute(&a).is_err());
        assert!(!a.output.exists());
        a.output = temp.path().join("out.csv");
        a.size_mode = Mode::Scan;
        assert!(execute(&a).is_err());
        a.size_mode = Mode::Estimate;
        a.start = Some("2".into());
        a.end = Some("1".into());
        assert!(execute(&a).is_err());
        a.start = None;
        a.end = None;
        a.users.clear();
        assert!(execute(&a).is_err());
        assert!(!a.output.exists());
        assert!(output_path(&temp.path().join("x:stream")).is_err());
    }

    #[test]
    fn dates_and_exact_identity_filters() {
        assert_eq!(
            timestamp("2025-01-01").unwrap(),
            timestamp("2025-01-01 00:00:00").unwrap()
        );
        assert_eq!(timestamp(" -1 ").unwrap(), -1);
        assert!(timestamp("bad").is_err());
        let temp = tempfile::tempdir().unwrap();
        let mut a = args(temp.path());
        let file = temp.path().join("chats.json");
        fs::write(
            &file,
            r#"[{"username":"alpha","chat_name":"same"},{"username":"beta","chat_name":"same"}]"#,
        )
        .unwrap();
        a.chats_json = Some(file);
        a.users.clear();
        assert_eq!(selected_chats(&a).unwrap().len(), 2);
        a.users = vec!["same".into()];
        assert!(selected_chats(&a).is_err());
        a.users = vec!["alpha".into(), "alpha".into()];
        assert!(selected_chats(&a).is_err());
    }
}
