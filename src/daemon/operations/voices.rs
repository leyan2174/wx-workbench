use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::history::{parse_time, parse_time_end};
use super::output::print_value;
use crate::adapters::wechat::media::{
    voice_catalog::MediaShard,
    voice_export::{self, Catalog, VoiceRow},
};
use crate::business::voice_export::{self as domain, Selection};
use crate::daemon::cache::DbCache;
use crate::daemon::query::{chat_type_of, load_names, Names};
use crate::infrastructure::publication::ExportTarget;
use crate::runtime::RuntimeContext;

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

pub use crate::service::operation_requests::voices::Args;

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
    let runtime = RuntimeContext::load()?;
    if let Some(expected) = std::env::var_os("WX_CLI_EXPECTED_RUNTIME") {
        anyhow::ensure!(
            expected.to_str() == Some(runtime.id.as_str()),
            "Account changed before voice export"
        );
    }
    let _config_pin = crate::service::config_pin::ConfigPin::new(&runtime)?;
    let rt = tokio::runtime::Runtime::new().context("创建运行时失败")?;
    let summary = rt.block_on(async {
        export_voices(
            &runtime,
            chat,
            PathBuf::from(output),
            Selection {
                limit,
                offset,
                since: since_ts,
                until: until_ts,
                ..Default::default()
            },
            overwrite,
        )
        .await
    })?;
    print_value(&summary, &super::output::resolve(json_output))
}

async fn export_voices(
    runtime: &RuntimeContext,
    chat: Option<String>,
    out_dir: PathBuf,
    selection: Selection<'_>,
    overwrite: bool,
) -> Result<Value> {
    let out_dir = std::path::absolute(out_dir)?;
    crate::infrastructure::publication::validate_export_target(runtime, &out_dir)?;
    let mut all_keys = crate::service::worker_keys::database_keys(runtime)?
        .ok_or(crate::key_store::Error::Missing)?;
    let media_paths = voice_export::media_database_paths(all_keys.0.keys());
    if media_paths.is_empty() {
        bail!("密钥库里没有 message/media_*.db 的密钥，请先运行 wx init --force");
    }
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("创建输出目录失败: {}", out_dir.display()))?;
    let output_guard = crate::attachment::local_files::HostOutputGuard::new(&out_dir)?;
    let summary_target =
        ExportTarget::capture(runtime, &out_dir.join("_voice_export_summary.json"))?;
    let db = DbCache::with_dirs(
        runtime.config.db_dir.clone(),
        runtime.cache_dir(),
        runtime.mtime_file(),
        std::mem::take(&mut all_keys.0),
    )
    .await?;
    let mut names = load_names(&db).await.unwrap_or_else(|_| Names {
        map: HashMap::new(),
        msg_db_keys: Vec::new(),
        biz_msg_db_keys: Vec::new(),
        verify_flags: HashMap::new(),
    });

    let target_username = chat
        .as_deref()
        .map(|name| {
            domain::resolve_chat(
                name,
                names
                    .map
                    .iter()
                    .map(|(user, display)| (user.as_str(), display.as_str())),
            )
        })
        .transpose()?;

    let mut shards = Vec::new();
    let mut missing_shards = Vec::new();
    for rel_key in media_paths {
        let Some(path) = db.get(&rel_key).await? else {
            missing_shards.push(rel_key);
            continue;
        };
        shards.push(MediaShard {
            source: rel_key,
            path,
        });
    }
    let source = Catalog::open(shards)?;
    let selected = domain::select(
        &source,
        &Selection {
            username: target_username.as_deref(),
            ..selection
        },
    );
    let scanned = selected.len();
    let mut exported = Vec::new();
    for entry in selected {
        exported.push(write_voice_row(
            runtime,
            &out_dir,
            &mut names,
            source.material(entry.slot)?,
            overwrite,
        )?);
    }

    let summary = json!({
        "output_dir": out_dir.to_string_lossy(),
        "chat_filter": chat,
        "target_username": target_username,
        "scanned_rows": scanned,
        "unmapped_rows": source.unmapped_rows,
        "missing_shards": missing_shards,
        "partial": source.unmapped_rows > 0 || !missing_shards.is_empty(),
        "exported": exported.len(),
        "items": exported,
    });
    summary_target.write_bytes_checked(&serde_json::to_vec_pretty(&summary)?, || {
        output_guard.verify()
    })?;
    Ok(summary)
}

fn write_voice_row(
    runtime: &RuntimeContext,
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
    crate::infrastructure::publication::validate_export_target(runtime, &chat_dir)?;
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

    let audio_target = capture_voice_target(runtime, &silk_path, overwrite)?;
    let evidence_target = capture_voice_target(runtime, &json_path, overwrite)?;

    let (silk, raw_had_0x02_prefix) = normalize_silk(&row.voice_data);
    let silk_header_ok = silk.starts_with(b"#!SILK_V3");
    audio_target.write_bytes(silk)?;

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
    evidence_target.write_bytes(&serde_json::to_vec_pretty(&exported)?)?;
    Ok(exported)
}

fn capture_voice_target(
    runtime: &RuntimeContext,
    path: &Path,
    overwrite: bool,
) -> Result<ExportTarget> {
    if overwrite {
        ExportTarget::capture(runtime, path)
    } else {
        ExportTarget::new_file(
            path,
            &crate::infrastructure::publication::export_protected(runtime),
        )
    }
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

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::fs;

    fn runtime(root: &Path) -> RuntimeContext {
        RuntimeContext {
            config: crate::config::Config {
                key_store: Some(root.join("store.dpapi")),
                db_dir: root.join("db"),
                keys_file: root.join("keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.into(),
            id: "synthetic".into(),
            directory: root.join("runtime"),
        }
    }
    fn names() -> Names {
        Names {
            map: HashMap::from([("chat".into(), "name".into())]),
            msg_db_keys: Vec::new(),
            biz_msg_db_keys: Vec::new(),
            verify_flags: HashMap::new(),
        }
    }
    fn row() -> VoiceRow {
        VoiceRow {
            chat_name_id: 1,
            chat_username: "chat".into(),
            create_time: 10,
            local_id: 2,
            svr_id: 3,
            data_index: String::new(),
            voice_data: b"\x02#!SILK_V3synthetic".to_vec(),
            media_db: "message/media_0.db".into(),
        }
    }

    #[test]
    fn fixed_runtime_raw_audio_evidence_and_no_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        fs::write(&runtime.config_path, b"changed invalid configuration").unwrap();
        let output = root.path().join("out");
        let first = write_voice_row(&runtime, &output, &mut names(), row(), false).unwrap();
        assert_eq!(fs::read(&first.audio_file).unwrap(), b"#!SILK_V3synthetic");
        let evidence: Value =
            serde_json::from_slice(&fs::read(&first.evidence_file).unwrap()).unwrap();
        assert_eq!(evidence["raw_had_0x02_prefix"], true);
        assert_eq!(evidence["voice_data_bytes"], row().voice_data.len());
        assert!(write_voice_row(&runtime, &output, &mut names(), row(), false).is_err());
        write_voice_row(&runtime, &output, &mut names(), row(), true).unwrap();
        assert_eq!(
            fs::read(&runtime.config_path).unwrap(),
            b"changed invalid configuration"
        );
    }

    #[test]
    fn all_artifacts_reject_account_paths_and_hardlink_aliases() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        for protected in [
            &runtime.config_path,
            &runtime.config.keys_file,
            runtime.config.key_store.as_ref().unwrap(),
        ] {
            fs::write(protected, b"protected synthetic bytes").unwrap();
            assert!(capture_voice_target(&runtime, protected, true).is_err());
        }
        for protected in [
            &runtime.config.db_dir,
            &runtime.config.decrypted_dir,
            &runtime.directory,
        ] {
            assert!(write_voice_row(&runtime, protected, &mut names(), row(), true).is_err());
            assert!(!protected.exists());
        }
        let output = root.path().join("out");
        fs::create_dir(&output).unwrap();
        for name in ["10_2.silk", "10_2.voice.json", "_voice_export_summary.json"] {
            let path = output.join(name);
            fs::hard_link(&runtime.config.keys_file, &path).unwrap();
            assert!(capture_voice_target(&runtime, &path, true).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"protected synthetic bytes");
        }
    }

    #[test]
    fn evidence_publication_failure_keeps_audio_and_old_evidence() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let output = root.path().join("out");
        let first = write_voice_row(&runtime, &output, &mut names(), row(), false).unwrap();
        fs::write(&first.audio_file, b"old audio").unwrap();
        fs::write(&first.evidence_file, b"old evidence").unwrap();
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&first.evidence_file)
            .unwrap();
        assert!(write_voice_row(&runtime, &output, &mut names(), row(), true).is_err());
        assert_eq!(fs::read(&first.audio_file).unwrap(), b"#!SILK_V3synthetic");
        assert_eq!(fs::read(&first.evidence_file).unwrap(), b"old evidence");
        assert_eq!(
            fs::read_dir(Path::new(&first.audio_file).parent().unwrap())
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn captured_summary_refuses_concurrent_replacement() {
        let root = tempfile::tempdir().unwrap();
        let runtime = runtime(root.path());
        let path = root.path().join("_voice_export_summary.json");
        fs::write(&path, b"old summary").unwrap();
        let summary = ExportTarget::capture(&runtime, &path).unwrap();
        fs::write(&path, b"concurrent summary").unwrap();
        assert!(summary.write_bytes(b"new summary").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"concurrent summary");
    }
}
