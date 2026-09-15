//! 原生语音管线的账号级字节适配；不写 WAV、不上传、不读取后端或凭据配置。
use super::{DbCache, Names};
use crate::toolkit::asr::database_media::{self, DatabaseVoice, DecryptedSource};
use crate::toolkit::asr::prepared_audio;
use anyhow::{ensure, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// 旧媒体 ID 入口的明确结果；不得把歧义当作首个命中或准备成功。
#[derive(Debug)]
pub(super) enum LegacyAudioResolution {
    Found(DatabaseVoice),
    AmbiguousMedia,
    AmbiguousMessage,
}

/// server 内部准备入口；local_id 是旧媒体 ID，不代表公开解码或转录成功。
/// limits 限制内层准备 JSON；调用方为外层 IPC 包装显式预留响应余量。
pub async fn q_prepare_voice(
    db: &DbCache,
    names: &Names,
    chat: &str,
    media_local_id: i64,
    limits: prepared_audio::Limits,
) -> Result<serde_json::Value> {
    limits
        .validate()
        .map_err(|_| anyhow::anyhow!("invalid voice preparation limits"))?;
    ensure!(media_local_id > 0, "voice media local_id must be positive");
    let voice = match resolve_legacy_audio(db, names, chat, media_local_id)
        .await
        .map_err(|_| anyhow::anyhow!("voice preparation unavailable"))?
    {
        LegacyAudioResolution::Found(voice) => voice,
        LegacyAudioResolution::AmbiguousMedia => anyhow::bail!("ambiguous voice media local_id"),
        LegacyAudioResolution::AmbiguousMessage => anyhow::bail!("ambiguous voice message"),
    };
    let bytes = prepared_audio::encode(&voice, limits)
        .map_err(|_| anyhow::anyhow!("voice preparation invalid or exceeds limits"))?;
    let prepared: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid prepared voice response"))?;
    Ok(serde_json::json!({"prepared_audio": prepared}))
}

/// chat 只接受精确 username 或唯一精确名称；local_id 始终是 VoiceInfo.local_id。
/// 使用完整账号源清单和共享反向关联器，不从媒体 ID 推测消息 ID。
pub(super) async fn resolve_legacy_audio(
    db: &DbCache,
    names: &Names,
    chat: &str,
    media_local_id: i64,
) -> Result<LegacyAudioResolution> {
    let username = super::q_resolve_chat(db, names, chat).await?;
    let before = inventory(db)?;
    let states = source_states(&before.paths)?;
    let mut sources = Vec::new();
    for (source, raw) in &before.keys {
        let path = db
            .get(raw)
            .await?
            .context("required legacy audio database unavailable")?;
        sources.push(DecryptedSource {
            source: source.clone(),
            path,
        });
    }
    let result = tokio::task::spawn_blocking(move || {
        database_media::resolve_voice_media_id(&sources, &username, media_local_id)
    })
    .await;
    let after = inventory(db)?;
    ensure!(
        before == after && states == source_states(&after.paths)?,
        "legacy audio source inventory or database changed"
    );
    match result? {
        Ok(voice) => Ok(LegacyAudioResolution::Found(voice)),
        Err(error) => match error.kind {
            database_media::ErrorKind::AmbiguousMedia => Ok(LegacyAudioResolution::AmbiguousMedia),
            database_media::ErrorKind::AmbiguousMessage => {
                Ok(LegacyAudioResolution::AmbiguousMessage)
            }
            _ => Err(error.into()),
        },
    }
}

fn canonical(key: &str) -> String {
    key.replace('\\', "/").to_ascii_lowercase()
}

#[derive(PartialEq, Eq)]
struct Inventory {
    keys: BTreeMap<String, String>,
    paths: Vec<PathBuf>,
}

fn inventory(db: &DbCache) -> Result<Inventory> {
    let mut keys = BTreeMap::new();
    for raw in db.raw_db_keys() {
        let key = canonical(&raw);
        if key == "contact/contact.db"
            || key.starts_with("message/message_")
            || key.starts_with("message/media_")
        {
            ensure!(
                keys.insert(key, raw).is_none(),
                "duplicate canonical audio source"
            );
        }
    }
    let paths = database_media::source_files(db.db_dir())?;
    let root = db.db_dir().canonicalize()?;
    let disk: BTreeSet<_> = paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .context("audio source outside selected account")?;
            Ok(canonical(
                relative.to_str().context("non-UTF8 audio source")?,
            ))
        })
        .collect::<Result<_>>()?;
    ensure!(
        disk == keys.keys().cloned().collect(),
        "unknown or missing audio database; complete account inventory required"
    );
    ensure!(
        keys.keys().any(|key| key.starts_with("message/media_")),
        "no media databases"
    );
    Ok(Inventory { keys, paths })
}

#[derive(PartialEq, Eq)]
struct FileState {
    identity: same_file::Handle,
    length: u64,
    modified: SystemTime,
}

fn file_state(path: &Path) -> Result<Option<FileState>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "invalid audio source file"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "reparse audio source file"
        );
    }
    Ok(Some(FileState {
        identity: same_file::Handle::from_path(path)?,
        length: metadata.len(),
        modified: metadata.modified()?,
    }))
}

fn source_states(paths: &[PathBuf]) -> Result<Vec<Option<FileState>>> {
    let mut states = Vec::new();
    for path in paths {
        states.push(Some(file_state(path)?.context("audio source disappeared")?));
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            states.push(file_state(Path::new(&sidecar))?);
        }
    }
    Ok(states)
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-audio/state_tests.rs"]
mod tests;
