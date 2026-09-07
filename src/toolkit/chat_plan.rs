//! 显式解密目录的只读导出计划；不发现账号、不读取配置、不导出消息。
use anyhow::{bail, ensure, Context, Result};
use chrono::{Local, TimeZone};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

pub const PLAN_CSV_FIELDS: [&str; 12] = [
    "export",
    "index",
    "username",
    "chat_name",
    "chat_type",
    "message_count",
    "first_time",
    "last_time",
    "attachment_estimated_bytes",
    "attachment_scanned_bytes",
    "total_estimated_bytes",
    "size_status",
];

/// 路径必须相对于 decrypted_dir。分片按调用方给定顺序累计。
#[derive(Debug, Default)]
pub struct PlanDatabases {
    pub message: Vec<PathBuf>,
    pub resource: Option<PathBuf>,
    pub media: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PlanChat {
    pub index: usize,
    pub username: String,
    pub chat_name: String,
    pub chat_type: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TimeRange {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeMode {
    Estimate,
    Scan,
}

/// source_dir 是账号源目录，media_dir 是显式 msg 目录，二者只指定一个。
/// 解密缓存仍通过 collect_plan_with_scan 的 decrypted_dir 参数传入，不重复扫描。
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub source_dir: Option<PathBuf>,
    pub media_dir: Option<PathBuf>,
    pub workers: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            source_dir: None,
            media_dir: None,
            workers: 1,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct MessageTableStats {
    pub message_count: i64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub message_body_bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanRow {
    pub export: String,
    pub index: usize,
    pub username: String,
    pub chat_name: String,
    pub chat_type: String,
    pub message_count: i64,
    pub message_body_bytes: i64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub first_time: String,
    pub last_time: String,
    pub attachment_estimated_bytes: i64,
    pub attachment_scanned_bytes: Option<i64>,
    pub total_estimated_bytes: i64,
    pub size_status: String,
}

fn safe_table(name: &str) -> bool {
    name.strip_prefix("Msg_")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn validate_range(range: TimeRange) -> Result<()> {
    ensure!(
        !matches!((range.start, range.end), (Some(a), Some(b)) if a > b),
        "开始时间不能晚于结束时间"
    );
    Ok(())
}

/// NULL 时间保留为 None；TEXT length 的字符计数语义与 Python SQLite 一致。
pub fn query_message_table_plan_stats(
    conn: &Connection,
    table: &str,
    range: TimeRange,
) -> Result<MessageTableStats> {
    validate_range(range)?;
    ensure!(safe_table(table), "非法消息表名");
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?;
    ensure!(exists, "消息表不存在或不是实体表");
    let sql = format!("SELECT COUNT(*), MIN(create_time), MAX(create_time), COALESCE(SUM(COALESCE(length(message_content),0) + COALESCE(length(compress_content),0) + COALESCE(length(packed_info_data),0)),0) FROM [{table}] WHERE (?1 IS NULL OR create_time >= ?1) AND (?2 IS NULL OR create_time <= ?2)");
    Ok(conn.query_row(&sql, params![range.start, range.end], |r| {
        Ok(MessageTableStats {
            message_count: r.get(0)?,
            first_ts: r.get(1)?,
            last_ts: r.get(2)?,
            message_body_bytes: r.get(3)?,
        })
    })?)
}

fn regular_component(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink(), "拒绝符号链接路径");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(metadata.file_attributes() & 0x400 == 0, "拒绝重解析点路径");
    }
    Ok(())
}

fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(!relative.as_os_str().is_empty(), "库路径不能为空");
    let mut path = root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            bail!("库路径必须是根目录内的相对路径")
        };
        ensure!(!name.to_string_lossy().contains(':'), "拒绝备用数据流路径");
        path.push(name);
        // 缺失文件交由统计层呈现 *_error，不创建数据库。
        if path.try_exists()? {
            regular_component(&path)?;
        }
    }
    if path.try_exists()? {
        ensure!(path.canonicalize()?.starts_with(root), "库路径越界");
        ensure!(path.is_file(), "库路径不是文件");
    }
    Ok(path)
}

fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    Ok(conn)
}

fn add(target: &mut i64, value: i64) -> Result<()> {
    *target = target.checked_add(value).context("计划统计数值溢出")?;
    Ok(())
}

fn format_time(ts: Option<i64>) -> Result<String> {
    match ts.filter(|t| *t != 0) {
        None => Ok(String::new()),
        Some(t) => Ok(Local
            .timestamp_opt(t, 0)
            .single()
            .context("时间戳超出支持范围")?
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()),
    }
}

const RESOURCE_SQL: &str = "SELECT COALESCE(SUM(COALESCE(d.size,0)),0) FROM ChatName2Id c JOIN MessageResourceInfo i ON i.chat_id=c.rowid LEFT JOIN MessageResourceDetail d ON d.message_id=i.message_id WHERE c.user_name=?1 AND (?2 IS NULL OR i.message_create_time>=?2) AND (?3 IS NULL OR i.message_create_time<=?3)";
const MEDIA_SQL: &str = "SELECT COALESCE(SUM(COALESCE(length(v.voice_data),0)),0) FROM Name2Id n JOIN VoiceInfo v ON v.chat_name_id=n.rowid WHERE n.user_name=?1 AND (?2 IS NULL OR v.create_time>=?2) AND (?3 IS NULL OR v.create_time<=?3)";

/// 兼容入口；Scan 未提供源目录时明确返回 scan_base_missing。
pub fn collect_plan(
    decrypted_dir: &Path,
    databases: &PlanDatabases,
    chats: &[PlanChat],
    range: TimeRange,
    mode: SizeMode,
) -> Result<Vec<PlanRow>> {
    if mode == SizeMode::Scan {
        return collect_plan_with_scan(
            decrypted_dir,
            databases,
            chats,
            range,
            &ScanOptions::default(),
        );
    }
    validate_range(range)?;
    ensure!(decrypted_dir.is_absolute(), "必须显式传入绝对解密目录");
    // 必须在 canonicalize 隐去 junction 之前逐级校验，并在全部查询期间固定祖先链。
    let (_root_pins, root_status) = pin_scan_root(decrypted_dir)?;
    ensure!(
        root_status.is_none(),
        "解密目录及祖先必须可访问且不含重解析点: {}",
        root_status.unwrap_or_default()
    );
    let root = decrypted_dir.canonicalize()?;
    ensure!(root.is_dir(), "解密目录不存在");
    let mut usernames = BTreeSet::new();
    for chat in chats {
        ensure!(
            !chat.username.is_empty() && usernames.insert(&chat.username),
            "username 不能为空或重复"
        );
    }
    let resolve = |paths: &[PathBuf]| -> Result<Vec<PathBuf>> {
        let mut seen = BTreeSet::new();
        paths
            .iter()
            .map(|p| {
                let p = checked_path(&root, p)?;
                ensure!(
                    seen.insert(p.to_string_lossy().to_lowercase()),
                    "重复数据库路径"
                );
                Ok(p)
            })
            .collect()
    };
    let messages = resolve(&databases.message)?;
    let media = resolve(&databases.media)?;
    let resource = databases
        .resource
        .as_ref()
        .map(|p| checked_path(&root, p))
        .transpose()?;
    let mut stats = vec![MessageTableStats::default(); chats.len()];
    let mut status = vec![BTreeSet::new(); chats.len()];
    let mut found = vec![false; chats.len()];
    for path in &messages {
        let conn = match open_readonly(path) {
            Ok(conn) => conn,
            Err(_) => {
                for s in &mut status {
                    s.insert("message_error");
                }
                continue;
            }
        };
        for (i, chat) in chats.iter().enumerate() {
            let table = format!("Msg_{:x}", md5::compute(chat.username.as_bytes()));
            let exists = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [&table],
                |r| r.get::<_, bool>(0),
            );
            match exists {
                Ok(false) => continue,
                Err(_) => {
                    status[i].insert("message_error");
                    continue;
                }
                Ok(true) => {}
            }
            match query_message_table_plan_stats(&conn, &table, range) {
                Ok(value) => {
                    found[i] = true;
                    add(&mut stats[i].message_count, value.message_count)?;
                    add(&mut stats[i].message_body_bytes, value.message_body_bytes)?;
                    // 旧实现跳过零时间戳；底层查询仍保留 Some(0)。
                    if let Some(t) = value.first_ts.filter(|t| *t != 0) {
                        stats[i].first_ts = Some(stats[i].first_ts.map_or(t, |old| old.min(t)));
                    }
                    if let Some(t) = value.last_ts.filter(|t| *t != 0) {
                        stats[i].last_ts = Some(stats[i].last_ts.map_or(t, |old| old.max(t)));
                    }
                }
                Err(_) => {
                    status[i].insert("message_error");
                }
            }
        }
    }
    for i in 0..chats.len() {
        if messages.is_empty() {
            status[i].insert("message_db_missing");
        } else if !found[i] {
            status[i].insert("no_message_table");
        }
    }
    let mut attachments = vec![0; chats.len()];
    if let Some(path) = resource {
        match attachment_values(&path, RESOURCE_SQL, chats, range) {
            Ok(values) => attachments = values,
            Err(_) => {
                for s in &mut status {
                    s.insert("resource_error");
                }
            }
        }
    } else {
        for s in &mut status {
            s.insert("resource_missing");
        }
    }
    if media.is_empty() {
        for s in &mut status {
            s.insert("media_missing");
        }
    }
    for path in &media {
        match attachment_values(path, MEDIA_SQL, chats, range) {
            Ok(values) => {
                for (total, value) in attachments.iter_mut().zip(values) {
                    add(total, value)?;
                }
            }
            Err(_) => {
                for s in &mut status {
                    s.insert("media_error");
                }
                // 与批量旧实现一致：保留已完成分片，首个失败后不继续。
                break;
            }
        }
    }
    chats
        .iter()
        .enumerate()
        .map(|(i, chat)| {
            let s = &stats[i];
            let mut total = s.message_body_bytes;
            add(&mut total, attachments[i])?;
            Ok(PlanRow {
                export: String::new(),
                index: chat.index,
                username: chat.username.clone(),
                chat_name: chat.chat_name.clone(),
                chat_type: chat.chat_type.clone(),
                message_count: s.message_count,
                message_body_bytes: s.message_body_bytes,
                first_ts: s.first_ts,
                last_ts: s.last_ts,
                first_time: format_time(s.first_ts)?,
                last_time: format_time(s.last_ts)?,
                attachment_estimated_bytes: attachments[i],
                attachment_scanned_bytes: None,
                total_estimated_bytes: total,
                size_status: if status[i].is_empty() {
                    "ok".into()
                } else {
                    format!(
                        "partial:{}",
                        status[i].iter().copied().collect::<Vec<_>>().join(",")
                    )
                },
            })
        })
        .collect()
}

/// 时间范围仅用于消息与估算；实际附件按旧契约扫描整个 username 目录。
pub fn collect_plan_with_scan(
    decrypted_dir: &Path,
    databases: &PlanDatabases,
    chats: &[PlanChat],
    range: TimeRange,
    options: &ScanOptions,
) -> Result<Vec<PlanRow>> {
    ensure!(
        (1..=6).contains(&options.workers),
        "扫描线程数必须在 1..=6 范围内"
    );
    ensure!(
        options.source_dir.is_none() || options.media_dir.is_none(),
        "source_dir 与 media_dir 只能指定一个"
    );
    let media = options
        .media_dir
        .clone()
        .or_else(|| options.source_dir.as_ref().map(|p| p.join("msg")));
    // 在启动工作线程之前验证整个源路径并持有所有祖先目录句柄。
    let (_pins, base_status) = match &media {
        Some(path) => pin_scan_root(path)?,
        None => (Vec::new(), Some("scan_base_missing")),
    };
    let mut rows = collect_plan(decrypted_dir, databases, chats, range, SizeMode::Estimate)?;
    if rows.is_empty() {
        return Ok(rows);
    }
    let workers = options.workers.min(rows.len());
    let chunk_size = rows.len().div_ceil(workers);
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for chunk in rows.chunks_mut(chunk_size) {
            let media = media.as_deref();
            handles.push(scope.spawn(move || {
                for row in chunk {
                    let mut scan = ScanTotal::default();
                    if let Some(status) = base_status {
                        scan.statuses.insert(status);
                    } else if let Some(media) = media {
                        scan = scan_username(media, &row.username);
                    }
                    row.attachment_scanned_bytes = Some(scan.bytes);
                    let mut statuses: BTreeSet<String> = row
                        .size_status
                        .strip_prefix("partial:")
                        .map(|s| s.split(',').map(str::to_owned).collect())
                        .unwrap_or_default();
                    statuses.extend(scan.statuses.into_iter().map(str::to_owned));
                    row.size_status = if statuses.is_empty() {
                        "ok".into()
                    } else {
                        format!(
                            "partial:{}",
                            statuses.into_iter().collect::<Vec<_>>().join(",")
                        )
                    };
                }
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("扫描工作线程异常终止"))?;
        }
        Ok(())
    })?;
    Ok(rows)
}

#[derive(Default)]
struct ScanTotal {
    bytes: i64,
    statuses: BTreeSet<&'static str>,
}

struct ScanPin {
    _handle: fs::File,
    metadata: fs::Metadata,
}

fn scan_pin(path: &Path) -> std::result::Result<ScanPin, &'static str> {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // 只读取属性、不跟随重解析点；拒绝共享删除，固定整个扫描链。
        options
            .access_mode(0x80)
            .share_mode(0x3)
            .custom_flags(0x02200000);
    }
    #[cfg(not(windows))]
    {
        options.read(true);
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("scan_reparse_skipped");
        }
    }
    let handle = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "scan_missing"
        } else {
            "scan_error"
        }
    })?;
    let metadata = handle.metadata().map_err(|_| "scan_error")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("scan_reparse_skipped");
        }
    }
    if metadata.file_type().is_symlink() {
        return Err("scan_reparse_skipped");
    }
    Ok(ScanPin {
        _handle: handle,
        metadata,
    })
}

fn pin_scan_root(path: &Path) -> Result<(Vec<ScanPin>, Option<&'static str>)> {
    ensure!(path.is_absolute(), "扫描目录必须是显式绝对路径");
    ensure!(
        path.as_os_str().to_string_lossy().encode_utf16().count() < 32760,
        "扫描路径过长"
    );
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                ensure!(
                    name.encode_utf16().count() <= 255
                        && !name
                            .chars()
                            .any(|c| c.is_control() || "<>:\"|?*".contains(c))
                        && !name.ends_with([' ', '.']),
                    "非法扫描目录组件"
                );
            }
            Component::ParentDir | Component::CurDir => bail!("扫描路径不能包含相对组件"),
            #[cfg(windows)]
            Component::Prefix(prefix) => {
                use std::path::Prefix;
                ensure!(
                    matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
                    "扫描仅支持本机磁盘绝对路径"
                );
            }
            _ => {}
        }
    }
    let mut current = PathBuf::new();
    let mut pins = Vec::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        match scan_pin(&current) {
            Ok(pin) if pin.metadata.is_dir() => pins.push(pin),
            Ok(_) => return Ok((pins, Some("scan_error"))),
            Err("scan_missing") => return Ok((pins, Some("scan_base_missing"))),
            Err(status) => return Ok((pins, Some(status))),
        }
    }
    Ok((pins, None))
}

fn scan_username(media: &Path, username: &str) -> ScanTotal {
    let mut total = ScanTotal::default();
    let hash = format!("{:x}", md5::compute(username.as_bytes()));
    for kind in ["attach", "file", "video"] {
        let parent = media.join(kind);
        let _parent_pin = match scan_pin(&parent) {
            Ok(pin) if pin.metadata.is_dir() => pin,
            Err("scan_missing") => continue,
            Ok(_) => {
                total.statuses.insert("scan_error");
                continue;
            }
            Err(status) => {
                total.statuses.insert(status);
                continue;
            }
        };
        let root = parent.join(&hash);
        match scan_pin(&root) {
            Ok(pin) if pin.metadata.is_dir() => scan_tree(&root, pin, 0, &mut total),
            Err("scan_missing") => {
                if kind != "attach" {
                    total.statuses.insert("scan_limited");
                }
            }
            Ok(_) => {
                total.statuses.insert("scan_error");
                if kind != "attach" {
                    total.statuses.insert("scan_limited");
                }
            }
            Err(status) => {
                total.statuses.insert(status);
            }
        }
    }
    total
}

fn scan_tree(path: &Path, _pin: ScanPin, depth: usize, total: &mut ScanTotal) {
    // 明确的深度边界，避免恶意目录耗尽线程栈；已完成部分仍可用。
    if depth >= 128 {
        total.statuses.insert("scan_depth_limited");
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            total.statuses.insert("scan_error");
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                total.statuses.insert("scan_error");
                continue;
            }
        };
        let child = entry.path();
        match scan_pin(&child) {
            Ok(pin) if pin.metadata.is_dir() => scan_tree(&child, pin, depth + 1, total),
            Ok(pin) if pin.metadata.is_file() => {
                // 按目录项计数，不按 inode 去重；硬链接、同内容副本均与旧版一致。
                match i64::try_from(pin.metadata.len())
                    .ok()
                    .and_then(|n| total.bytes.checked_add(n))
                {
                    Some(bytes) => total.bytes = bytes,
                    None => {
                        total.statuses.insert("scan_overflow");
                    }
                }
            }
            Ok(_) => {
                total.statuses.insert("scan_error");
            }
            Err(status) => {
                total.statuses.insert(if status == "scan_missing" {
                    "scan_error"
                } else {
                    status
                });
            }
        }
    }
}

fn attachment_values(
    path: &Path,
    sql: &str,
    chats: &[PlanChat],
    range: TimeRange,
) -> Result<Vec<i64>> {
    let conn = open_readonly(path)?;
    let mut statement = conn.prepare(sql)?;
    chats
        .iter()
        .map(|chat| {
            Ok(
                statement
                    .query_row(params![chat.username, range.start, range.end], |r| r.get(0))?,
            )
        })
        .collect()
}

/// 与 csv.DictWriter 的 UTF-8-SIG、CRLF、最小引号规则保持一致。
pub fn render_plan_csv(rows: &[PlanRow]) -> Result<Vec<u8>> {
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut bytes);
        writer.write_record(PLAN_CSV_FIELDS)?;
        for row in rows {
            writer.write_record([
                row.export.clone(),
                row.index.to_string(),
                row.username.clone(),
                row.chat_name.clone(),
                row.chat_type.clone(),
                row.message_count.to_string(),
                row.first_time.clone(),
                row.last_time.clone(),
                row.attachment_estimated_bytes.to_string(),
                row.attachment_scanned_bytes
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                row.total_estimated_bytes.to_string(),
                row.size_status.clone(),
            ])?;
        }
        writer.flush()?;
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "chat_plan_tests.rs"]
mod tests;
