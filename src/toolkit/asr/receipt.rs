//! 强语音身份索引；沿调用方 runtime.id 隔离，不认证同路径替换的账号数据。
//! 可信缓存中的 receipt 不是签名；只证明历史成功记录，不证明源消息仍存在。
use super::{
    cache::{Cache, CacheKey, CachedTranscription, ConfigIdentity},
    cached::{self, CacheState, CachedOutcome, CachedRequest},
    database_media::{VoiceEvidence, MAX_VOICE_BYTES},
    Backend, Transcription,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

const FIELD: &str = "_wx_asr_receipts";
const MAX_IDENTITIES: usize = 65_536;
const MAX_CONFIGURATIONS: usize = 64;

pub enum LookupOutcome {
    Hit(CachedOutcome),
    Miss,
    Conflict,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptState {
    Stored,
    AlreadyPresent,
    Conflict,
    Unavailable,
}

/// 只读查询；授权先于缓存/模型 IO，不解码、不转录、不创建文件、不自动迁移。
/// username 必须由调用方精确绑定；返回后宿主仍须复核账号、取消和响应预算。
pub fn lookup_success(
    cache_path: &Path,
    account: &str,
    exact_username: &str,
    media_id: i64,
    backend: &Backend,
) -> LookupOutcome {
    lookup(cache_path, account, exact_username, media_id, backend)
        .unwrap_or(LookupOutcome::Unavailable)
}

fn lookup(
    cache_path: &Path,
    account: &str,
    username: &str,
    media_id: i64,
    backend: &Backend,
) -> Result<LookupOutcome> {
    backend.check_authorization()?;
    validate_lookup(username, media_id)?;
    let data = Cache::open(cache_path, account)?.receipt_data()?;
    let index = Index::read(&data)?;
    let Some(receipt) = index.entries.get(&index_key(username, media_id)?) else {
        return Ok(LookupOutcome::Miss);
    };
    if receipt.conflict {
        return Ok(LookupOutcome::Conflict);
    }
    // 与普通转录共用真实配置算法；句柄保持至缓存结果校验完毕。
    let (config, _files, _prepared) =
        cached::identity(backend, receipt.proof.evidence.create_time)?;
    let Some(expected_record) = receipt.configurations.get(config.fingerprint()) else {
        return Ok(LookupOutcome::Miss);
    };
    let Some(record) = Cache::lookup_value(&data, &receipt.proof.key(&config)?)? else {
        return Ok(LookupOutcome::Miss);
    };
    ensure!(
        record.create_time == Some(receipt.proof.evidence.create_time)
            && record_digest(&record)? == *expected_record,
        "receipt record mismatch"
    );
    Ok(LookupOutcome::Hit(CachedOutcome {
        transcription: Transcription {
            text: record.text,
            language: record.language,
            backend: cached::backend_name(backend).into(),
        },
        create_time: receipt.proof.evidence.create_time,
        cache_state: CacheState::Hit,
    }))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    #[serde(with = "Evidence")]
    evidence: VoiceEvidence,
    audio_sha256: String,
    audio_bytes: usize,
}

impl Proof {
    pub(super) fn new(request: &CachedRequest<'_>, evidence: &VoiceEvidence) -> Result<Self> {
        ensure!(
            request.username == evidence.username
                && request.source == evidence.message_source
                && request.local_id == evidence.message_local_id
                && request.create_time == evidence.create_time,
            "receipt request evidence mismatch"
        );
        let proof = Self {
            evidence: evidence.clone(),
            audio_sha256: digest(request.silk),
            audio_bytes: request.silk.len(),
        };
        proof.validate()?;
        Ok(proof)
    }

    pub(super) fn key(&self, config: &ConfigIdentity) -> Result<CacheKey> {
        CacheKey::from_audio_sha256(
            &self.evidence.username,
            &self.evidence.message_source,
            self.evidence.message_local_id,
            &self.audio_sha256,
            config,
        )
    }

    fn validate(&self) -> Result<()> {
        let e = &self.evidence;
        validate_lookup(&e.username, e.media_local_id)?;
        let source = |s: &str, prefix: &str| {
            s.len() <= 128
                && s.strip_prefix(prefix)
                    .and_then(|v| v.strip_suffix(".db"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        };
        ensure!(
            source(&e.message_source, "message/message_")
                && source(&e.media_source, "message/media_")
                && e.message_table == format!("Msg_{:x}", md5::compute(e.username.as_bytes()))
                && e.message_local_id > 0
                && e.server_id != 0
                && self.audio_bytes > 0
                && self.audio_bytes <= MAX_VOICE_BYTES
                && is_digest(&self.audio_sha256),
            "invalid receipt evidence"
        );
        Ok(())
    }
}

// 描述已有证据的存储形状；不复制 SQL 关联或从媒体 ID 推断消息 ID。
#[derive(Serialize, Deserialize)]
#[serde(remote = "VoiceEvidence", deny_unknown_fields)]
struct Evidence {
    username: String,
    message_source: String,
    message_table: String,
    message_local_id: i64,
    server_id: i64,
    create_time: i64,
    media_source: String,
    media_rowid: i64,
    media_chat_name_id: i64,
    media_local_id: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u8,
    entries: BTreeMap<String, Receipt>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    proof: Proof,
    // 同一强证据可拥有多个配置结果；值为成功记录摘要，不保存文本副本。
    configurations: BTreeMap<String, String>,
    conflict: bool,
}

impl Index {
    fn read(data: &Value) -> Result<Self> {
        let Some(value) = data.get(FIELD) else {
            return Ok(Self {
                version: 1,
                entries: BTreeMap::new(),
            });
        };
        let index: Self = serde_json::from_value(value.clone())?;
        ensure!(
            index.version == 1 && index.entries.len() <= MAX_IDENTITIES,
            "unsupported receipt index"
        );
        for (key, receipt) in &index.entries {
            receipt.proof.validate()?;
            ensure!(
                *key == index_key(
                    &receipt.proof.evidence.username,
                    receipt.proof.evidence.media_local_id
                )? && !receipt.configurations.is_empty()
                    && receipt.configurations.len() <= MAX_CONFIGURATIONS
                    && receipt
                        .configurations
                        .iter()
                        .all(|(config, record)| is_digest(config) && is_digest(record)),
                "invalid receipt index"
            );
        }
        Ok(index)
    }
}

/// 仅由 Cache 的既有加锁事务调用；索引与 entries 共用一次 publish。
pub(super) fn merge(
    data: &mut Value,
    proof: &Proof,
    config: &ConfigIdentity,
    stored: &CachedTranscription,
    record_matches: bool,
) -> Result<(ReceiptState, bool)> {
    proof.validate()?;
    let mut index = Index::read(data)?;
    let key = index_key(&proof.evidence.username, proof.evidence.media_local_id)?;
    let record = record_digest(stored)?;
    let config = config.fingerprint();
    let mut changed = false;
    if !index.entries.contains_key(&key) {
        ensure!(index.entries.len() < MAX_IDENTITIES, "receipt index limit");
        index.entries.insert(
            key.clone(),
            Receipt {
                proof: proof.clone(),
                configurations: BTreeMap::from([(config.to_owned(), record.clone())]),
                conflict: false,
            },
        );
        changed = true;
    }
    let receipt = index.entries.get_mut(&key).expect("receipt inserted");
    let conflicting = receipt.proof != *proof
        || !record_matches
        || stored.create_time != Some(proof.evidence.create_time)
        || receipt
            .configurations
            .get(config)
            .is_some_and(|v| *v != record);
    if conflicting && !receipt.conflict {
        // 保留原证据并永久标记冲突；拒绝新关联不能让旧关联继续伪装为唯一。
        receipt.conflict = true;
        changed = true;
    }
    if !receipt.conflict && !receipt.configurations.contains_key(config) {
        ensure!(
            receipt.configurations.len() < MAX_CONFIGURATIONS,
            "receipt configuration limit"
        );
        receipt.configurations.insert(config.to_owned(), record);
        changed = true;
    }
    let state = if receipt.conflict {
        ReceiptState::Conflict
    } else if changed {
        ReceiptState::Stored
    } else {
        ReceiptState::AlreadyPresent
    };
    if changed {
        data[FIELD] = serde_json::to_value(index)?;
    }
    Ok((state, changed))
}

fn validate_lookup(username: &str, media_id: i64) -> Result<()> {
    ensure!(
        !username.trim().is_empty()
            && username.len() <= 1024
            && !username.chars().any(char::is_control)
            && media_id > 0,
        "explicit receipt identity required"
    );
    Ok(())
}

fn index_key(username: &str, media_id: i64) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(username, media_id))?))
}

fn record_digest(record: &CachedTranscription) -> Result<String> {
    Ok(digest(&serde_json::to_vec(record)?))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod tests;
