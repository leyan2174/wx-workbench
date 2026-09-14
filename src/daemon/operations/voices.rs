use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::history::{parse_time, parse_time_end};
use super::output::print_value;
use crate::config;
use crate::daemon::cache::DbCache;
use crate::daemon::query::{chat_type_of, load_names, Names};

#[derive(Debug, Clone)]
struct VoiceRow {
    chat_name_id: i64,
    chat_username: String,
    create_time: i64,
    local_id: i64,
    svr_id: i64,
    data_index: String,
    voice_data: Vec<u8>,
    media_db: String,
}

#[derive(Debug, Serialize)]
struct ExportedVoice {
    chat: String,
    chat_username: String,
    chat_type: String,
    timestamp: i64,
    time: String,
    local_id: i64,
    svr_id: i64,
    chat_name_id: i64,
    data_index: String,
    media_db: String,
    audio_file: String,
    evidence_file: String,
    audio_format: String,
    voice_data_bytes: usize,
    raw_had_0x02_prefix: bool,
    silk_header_ok: bool,
}

/// CLI 解析和后台执行共用字段定义；IPC 仍保留原来的扁平字段。
#[derive(clap::Args)]
pub struct Args {
    /// 会话名称（可选；省略则导出全部语音）
    pub chat: Option<String>,
    /// 输出目录
    #[arg(short = 'o', long)]
    pub output: String,
    /// 最多导出条数
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,
    /// 分页偏移
    #[arg(long, default_value = "0")]
    pub offset: usize,
    /// 起始时间 YYYY-MM-DD
    #[arg(long)]
    pub since: Option<String>,
    /// 结束时间 YYYY-MM-DD
    #[arg(long)]
    pub until: Option<String>,
    /// 目标已存在时覆盖
    #[arg(long)]
    pub overwrite: bool,
    /// 输出 JSON（默认 YAML）
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_voices(args: Args) -> Result<()> {
    let Args {
        chat,
        output,
        limit,
        offset,
        since,
        until,
        overwrite,
        json: json_output,
    } = args;
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;
    let rt = tokio::runtime::Runtime::new().context("创建运行时失败")?;
    let summary = rt.block_on(async {
        export_voices(
            chat,
            PathBuf::from(output),
            limit,
            offset,
            since_ts,
            until_ts,
            overwrite,
        )
        .await
    })?;
    print_value(&summary, &super::output::resolve(json_output))
}

async fn export_voices(
    chat: Option<String>,
    out_dir: PathBuf,
    limit: Option<usize>,
    offset: usize,
    since: Option<i64>,
    until: Option<i64>,
    overwrite: bool,
) -> Result<Value> {
    let cfg = config::load_config()?;
    let all_keys = load_all_keys(&cfg.keys_file)?;
    let media_keys = media_db_keys(&all_keys);
    if media_keys.is_empty() {
        bail!("all_keys.json 里没有 message/media_*.db 的密钥，请先运行 wx init --force");
    }

    let db = DbCache::new(cfg.db_dir.clone(), all_keys).await?;
    let mut names = load_names(&db).await.unwrap_or_else(|_| Names {
        map: HashMap::new(),
        md5_to_uname: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    });

    let target_username = chat
        .as_deref()
        .map(|name| resolve_chat(name, &names))
        .transpose()?;

    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("创建输出目录失败: {}", out_dir.display()))?;

    let mut exported = Vec::new();
    let mut scanned = 0usize;
    for rel_key in media_keys {
        let Some(path) = db.get(&rel_key).await? else {
            continue;
        };
        let mut rows = read_voice_rows(
            &path,
            &rel_key,
            target_username.as_deref(),
            since,
            until,
            offset,
            limit,
        )?;
        scanned += rows.len();
        for row in rows.drain(..) {
            if limit.is_some_and(|n| exported.len() >= n) {
                break;
            }
            let item = write_voice_row(&out_dir, &mut names, row, overwrite)?;
            exported.push(item);
        }
        if limit.is_some_and(|n| exported.len() >= n) {
            break;
        }
    }

    let summary_path = out_dir.join("_voice_export_summary.json");
    let summary = json!({
        "output_dir": out_dir.to_string_lossy(),
        "chat_filter": chat,
        "target_username": target_username,
        "scanned_rows": scanned,
        "exported": exported.len(),
        "items": exported,
    });
    std::fs::write(&summary_path, serde_json::to_vec_pretty(&summary)?)?;
    Ok(summary)
}

fn load_all_keys(path: &Path) -> Result<HashMap<String, String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取密钥文件失败: {}", path.display()))?;
    let raw: Value = serde_json::from_str(&text).context("all_keys.json 格式错误")?;
    let mut result = HashMap::new();
    if let Some(obj) = raw.as_object() {
        for (key, value) in obj {
            if key.starts_with('_') {
                continue;
            }
            let enc_key = value
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    value
                        .as_object()
                        .and_then(|o| o.get("enc_key"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            if !enc_key.is_empty() {
                result.insert(key.replace('\\', "/"), enc_key);
            }
        }
    }
    Ok(result)
}

fn media_db_keys(keys: &HashMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = keys
        .keys()
        .filter(|key| {
            let key = key.replace('\\', "/");
            key.starts_with("message/media_") && key.ends_with(".db")
        })
        .cloned()
        .collect();
    out.sort();
    out
}

fn resolve_chat(chat: &str, names: &Names) -> Result<String> {
    if names.map.contains_key(chat) {
        return Ok(chat.to_string());
    }
    let needle = chat.to_lowercase();
    let matches: Vec<_> = names
        .map
        .iter()
        .filter(|(username, display)| {
            username.to_lowercase().contains(&needle) || display.to_lowercase().contains(&needle)
        })
        .map(|(username, _)| username.clone())
        .collect();

    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("找不到会话: {}", chat),
        many => bail!(
            "会话名称不唯一: {}，匹配到 {} 个，请使用 wxid 或 @chatroom",
            chat,
            many.len()
        ),
    }
}

fn read_voice_rows(
    media_db_path: &Path,
    media_db: &str,
    target_username: Option<&str>,
    since: Option<i64>,
    until: Option<i64>,
    offset: usize,
    limit: Option<usize>,
) -> Result<Vec<VoiceRow>> {
    let conn = Connection::open(media_db_path)
        .with_context(|| format!("打开语音库失败: {}", media_db_path.display()))?;
    let id_map = read_name2id(&conn)?;
    let mut sql = String::from(
        "SELECT chat_name_id, create_time, local_id, svr_id, COALESCE(data_index, ''), voice_data \
         FROM VoiceInfo WHERE voice_data IS NOT NULL",
    );
    if since.is_some() {
        sql.push_str(" AND create_time >= ?");
    }
    if until.is_some() {
        sql.push_str(" AND create_time <= ?");
    }
    sql.push_str(" ORDER BY create_time, local_id");

    let mut stmt = conn.prepare(&sql)?;
    let mut params = Vec::new();
    if let Some(v) = since {
        params.push(v);
    }
    if let Some(v) = until {
        params.push(v);
    }
    let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let chat_name_id: i64 = row.get(0)?;
        let Some(chat_username) = id_map.get(&chat_name_id).cloned() else {
            continue;
        };
        if target_username.is_some_and(|target| target != chat_username) {
            continue;
        }
        out.push(VoiceRow {
            chat_name_id,
            chat_username,
            create_time: row.get(1)?,
            local_id: row.get(2)?,
            svr_id: row.get(3).unwrap_or(0),
            data_index: row.get(4).unwrap_or_default(),
            voice_data: row.get(5)?,
            media_db: media_db.to_string(),
        });
    }

    let start = offset.min(out.len());
    let end = limit
        .map(|n| start.saturating_add(n).min(out.len()))
        .unwrap_or(out.len());
    Ok(out[start..end].to_vec())
}

fn read_name2id(conn: &Connection) -> Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT rowid, user_name FROM Name2Id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

fn write_voice_row(
    out_root: &Path,
    names: &mut Names,
    row: VoiceRow,
    overwrite: bool,
) -> Result<ExportedVoice> {
    let chat_type = chat_type_of(&row.chat_username, names).to_string();
    let lane = if chat_type == "group" {
        "群聊"
    } else {
        "一对一聊天"
    };
    let display = names.display(&row.chat_username);
    let safe_display = sanitize_path_component(&display);
    let chat_dir = out_root.join(lane).join(safe_display);
    std::fs::create_dir_all(&chat_dir)?;

    let stem = format!("{}_{}", row.create_time, row.local_id);
    let silk_path = chat_dir.join(format!("{stem}.silk"));
    let json_path = chat_dir.join(format!("{stem}.voice.json"));
    if !overwrite && (silk_path.exists() || json_path.exists()) {
        bail!(
            "目标已存在: {}（需要覆盖请加 --overwrite）",
            silk_path.display()
        );
    }

    let (silk, raw_had_0x02_prefix) = normalize_silk(&row.voice_data);
    let silk_header_ok = silk.starts_with(b"#!SILK_V3");
    std::fs::write(&silk_path, silk)?;

    let time = fmt_time(row.create_time);
    let exported = ExportedVoice {
        chat: display,
        chat_username: row.chat_username,
        chat_type,
        timestamp: row.create_time,
        time,
        local_id: row.local_id,
        svr_id: row.svr_id,
        chat_name_id: row.chat_name_id,
        data_index: row.data_index,
        media_db: row.media_db,
        audio_file: silk_path.to_string_lossy().into_owned(),
        evidence_file: json_path.to_string_lossy().into_owned(),
        audio_format: "silk".to_string(),
        voice_data_bytes: row.voice_data.len(),
        raw_had_0x02_prefix,
        silk_header_ok,
    };
    std::fs::write(&json_path, serde_json::to_vec_pretty(&exported)?)?;
    Ok(exported)
}

fn normalize_silk(data: &[u8]) -> (&[u8], bool) {
    if data.first() == Some(&0x02) && data.get(1..).is_some_and(|d| d.starts_with(b"#!SILK_V3")) {
        (&data[1..], true)
    } else {
        (data, false)
    }
}

fn sanitize_path_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed
    }
}

fn fmt_time(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}
