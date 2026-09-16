//! 显式解密目录的只读导出计划；不发现账号、不读取配置、不导出消息。
#[cfg(test)]
use crate::adapters::wechat::planning::scan::{scan_pin, scan_tree};
use crate::adapters::wechat::planning::{
    self,
    scan::{media_root, pin_scan_root, scan_username, ScanTotal},
    SqliteSource,
};
use crate::business::chat_plan::{self as domain, Partial, Plan, PlanChat, SizeMode, TimeRange};
use anyhow::{ensure, Context, Result};
use chrono::{Local, TimeZone};
#[cfg(test)]
use domain::add;
#[cfg(test)]
use domain::MessageStatistics as MessageTableStats;
#[cfg(test)]
use planning::query_message_table_plan_stats;
use planning::PlanDatabases;
#[cfg(test)]
use rusqlite::Connection;
use serde::Serialize;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

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

fn partial_code(status: Partial) -> &'static str {
    match status {
        Partial::MediaReadFailed => "media_error",
        Partial::MediaSourceMissing => "media_missing",
        Partial::MessageSourceMissing => "message_db_missing",
        Partial::MessageReadFailed => "message_error",
        Partial::ConversationAbsent => "no_message_table",
        Partial::ResourceReadFailed => "resource_error",
        Partial::ResourceSourceMissing => "resource_missing",
        Partial::ScanBaseMissing => "scan_base_missing",
        Partial::ScanDepthLimited => "scan_depth_limited",
        Partial::ScanError => "scan_error",
        Partial::ScanLimited => "scan_limited",
        Partial::ScanMissing => "scan_missing",
        Partial::ScanOverflow => "scan_overflow",
        Partial::ScanReparseSkipped => "scan_reparse_skipped",
    }
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

fn read_plan(
    decrypted_dir: &Path,
    databases: &PlanDatabases,
    chats: &[PlanChat],
    range: TimeRange,
) -> Result<Plan> {
    domain::validate_range(range)?;
    ensure!(decrypted_dir.is_absolute(), "必须显式传入绝对解密目录");
    let (_root_pins, root_status) = pin_scan_root(decrypted_dir)?;
    ensure!(
        root_status.is_none(),
        "解密目录及祖先必须可访问且不含重解析点: {}",
        root_status.map(partial_code).unwrap_or_default()
    );
    let root = decrypted_dir.canonicalize()?;
    ensure!(root.is_dir(), "解密目录不存在");
    let mut source = SqliteSource::new(&root, databases)?;
    Ok(Plan::read(&mut source, chats, range)?)
}

fn project_plan(plan: Plan) -> Result<Vec<PlanRow>> {
    plan.rows
        .into_iter()
        .map(|row| {
            let size_status = if row.statuses.is_empty() {
                "ok".into()
            } else {
                let mut labels: Vec<_> = row.statuses.into_iter().map(partial_code).collect();
                labels.sort_unstable();
                format!("partial:{}", labels.join(","))
            };
            Ok(PlanRow {
                export: String::new(),
                index: row.chat.index,
                username: row.chat.username,
                chat_name: row.chat.chat_name,
                chat_type: row.chat.chat_type,
                message_count: row.messages.message_count,
                message_body_bytes: row.messages.message_body_bytes,
                first_ts: row.messages.first_ts,
                last_ts: row.messages.last_ts,
                first_time: format_time(row.messages.first_ts)?,
                last_time: format_time(row.messages.last_ts)?,
                attachment_estimated_bytes: row.attachment_estimated_bytes,
                attachment_scanned_bytes: row.attachment_scanned_bytes,
                total_estimated_bytes: row.total_estimated_bytes,
                size_status,
            })
        })
        .collect()
}

/// Compatibility scan without an explicit media root reports scan_base_missing.
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
    project_plan(read_plan(decrypted_dir, databases, chats, range)?)
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
    let media = media_root(options.source_dir.as_deref(), options.media_dir.as_deref());
    // 在启动工作线程之前验证整个源路径并持有所有祖先目录句柄。
    let (_pins, base_status) = match &media {
        Some(path) => pin_scan_root(path)?,
        None => (Vec::new(), Some(Partial::ScanBaseMissing)),
    };
    let mut plan = read_plan(decrypted_dir, databases, chats, range)?;
    let rows = &mut plan.rows;
    if rows.is_empty() {
        return project_plan(plan);
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
                        scan = scan_username(media, &row.chat.username);
                    }
                    row.apply_scan(scan);
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
    project_plan(plan)
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
#[path = "chat_export_plan_tests.rs"]
mod tests;
