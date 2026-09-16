//! MCP 转录持久缓存核心；调用方显式提供路径、账号及音频/配置身份。
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// 仅保存摘要；不得传入 api_key。模型身份应包含内容摘要或不可变版本。
pub struct ConfigIdentity(String);
impl ConfigIdentity {
    pub(super) fn fingerprint(&self) -> &str {
        &self.0
    }

    pub fn new(
        backend: &str,
        model_identity: &str,
        language: &str,
        options_identity: &str,
    ) -> Result<Self> {
        ensure!(
            matches!(
                backend,
                "whisper_cpp" | "python_whisper" | "openai_compatible"
            ),
            "unsupported cache backend"
        );
        ensure!(
            !model_identity.trim().is_empty() && !language.trim().is_empty(),
            "explicit model and language identities required"
        );
        Ok(Self(digest(&serde_json::to_vec(&(
            backend,
            model_identity,
            language,
            options_identity,
        ))?)))
    }
}

pub struct CacheKey(String);
impl CacheKey {
    pub fn new(
        username: &str,
        source: &str,
        local_id: i64,
        audio: &[u8],
        config: &ConfigIdentity,
    ) -> Result<Self> {
        ensure!(!audio.is_empty(), "audio identity requires nonempty audio");
        Self::from_audio_sha256(username, source, local_id, &digest(audio), config)
    }

    /// 上层可持久保存已验证摘要，在源音频清理后继续查询；不推断缺失身份。
    pub fn from_audio_sha256(
        username: &str,
        source: &str,
        local_id: i64,
        audio_sha256: &str,
        config: &ConfigIdentity,
    ) -> Result<Self> {
        ensure!(
            !username.trim().is_empty()
                && !source.trim().is_empty()
                && !source.eq_ignore_ascii_case("unknown")
                && local_id > 0
                && audio_sha256.len() == 64
                && audio_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "explicit voice and audio identity required"
        );
        Ok(Self(digest(&serde_json::to_vec(&(
            username,
            source,
            local_id,
            audio_sha256.to_ascii_lowercase(),
            &config.0,
        ))?)))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedTranscription {
    pub text: String,
    pub language: String,
    pub create_time: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreStatus {
    Stored,
    AlreadyPresent,
}

pub struct Cache {
    path: PathBuf,
    account: String,
}
impl Cache {
    pub fn open(path: &Path, account: &str) -> Result<Self> {
        ensure!(
            !account.trim().is_empty(),
            "explicit account identity required"
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let path =
            fs::canonicalize(parent)?.join(path.file_name().context("cache filename required")?);
        let cache = Self {
            path,
            account: digest(account.as_bytes()),
        };
        cache.load()?;
        Ok(cache)
    }

    fn load(&self) -> Result<(Option<Snapshot>, Value)> {
        let snapshot = Snapshot::read(&self.path)?;
        let value = match &snapshot {
            Some(snapshot) => serde_json::from_slice(&snapshot.bytes)
                .context("invalid cache JSON; original preserved")?,
            None => {
                json!({"_wx_asr_cache":{"version":1,"account_sha256":self.account},"entries":{}})
            }
        };
        ensure!(
            value
                .pointer("/_wx_asr_cache/version")
                .and_then(Value::as_u64)
                == Some(1),
            "unscoped legacy or unsupported cache; explicit migration required"
        );
        ensure!(
            value
                .pointer("/_wx_asr_cache/account_sha256")
                .and_then(Value::as_str)
                == Some(self.account.as_str()),
            "cache belongs to another account"
        );
        ensure!(
            value.get("entries").is_some_and(Value::is_object),
            "invalid cache entries; original preserved"
        );
        Ok((snapshot, value))
    }

    pub fn lookup(&self, key: &CacheKey) -> Result<Option<CachedTranscription>> {
        let (_, value) = self.load()?;
        Self::lookup_value(&value, key)
    }

    // receipt 与成功记录必须来自同一次已校验的缓存读取。
    pub(super) fn receipt_data(&self) -> Result<Value> {
        Ok(self.load()?.1)
    }

    pub(super) fn lookup_value(
        value: &Value,
        key: &CacheKey,
    ) -> Result<Option<CachedTranscription>> {
        match value["entries"].get(&key.0) {
            None => Ok(None),
            Some(entry) => Ok(Some(
                serde_json::from_value(entry.clone())
                    .context("unrecognized cache record; original preserved")?,
            )),
        }
    }

    /// 只接收成功识别结果；持久化错误由调用方独立处理，不改变识别结果。
    pub fn store_success(
        &self,
        key: &CacheKey,
        result: &CachedTranscription,
    ) -> Result<StoreStatus> {
        self.store_success_checked(key, result, || Ok(()))
    }

    /// 暂存与快照复核完成后，允许宿主在发布前拒绝提交。
    pub fn store_success_checked(
        &self,
        key: &CacheKey,
        result: &CachedTranscription,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<StoreStatus> {
        Ok(self.store_checked(key, result, None, before_commit)?.0)
    }

    pub(super) fn store_success_with_receipt_checked(
        &self,
        key: &CacheKey,
        result: &CachedTranscription,
        proof: &super::receipt::Proof,
        config: &ConfigIdentity,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<(StoreStatus, super::receipt::ReceiptState)> {
        ensure!(key.0 == proof.key(config)?.0, "receipt key mismatch");
        let (stored, receipt) =
            self.store_checked(key, result, Some((proof, config)), before_commit)?;
        Ok((stored, receipt.expect("receipt requested")))
    }

    fn store_checked(
        &self,
        key: &CacheKey,
        result: &CachedTranscription,
        receipt: Option<(&super::receipt::Proof, &ConfigIdentity)>,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<(StoreStatus, Option<super::receipt::ReceiptState>)> {
        let _lock = Lock::acquire(&self.path)?;
        let (snapshot, mut value) = self.load()?;
        // 保留命中记录及其未知字段；不覆盖同键的未知/损坏条目。
        let (status, stored) = if let Some(existing) = value["entries"].get(&key.0) {
            let stored: CachedTranscription = serde_json::from_value(existing.clone())
                .context("unrecognized cache record; overwrite refused")?;
            (StoreStatus::AlreadyPresent, stored)
        } else {
            value["entries"][&key.0] = serde_json::to_value(result)?;
            (StoreStatus::Stored, result.clone())
        };
        let mut changed = status == StoreStatus::Stored;
        let receipt_status = if let Some((proof, config)) = receipt {
            let (state, updated) =
                super::receipt::merge(&mut value, proof, config, &stored, stored == *result)?;
            changed |= updated;
            Some(state)
        } else {
            None
        };
        if changed {
            publish(&self.path, snapshot, &value, before_commit)?;
        }
        Ok((status, receipt_status))
    }
}

// 同目录协议锁只协调遵守协议的写者；崩溃残留不自动删除。
struct Lock {
    file: Option<fs::File>,
    path: PathBuf,
}
impl Lock {
    fn acquire(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .context("cache filename required")?
            .to_string_lossy()
            .to_lowercase();
        let path = path.with_file_name(format!(".{name}.asr-cache.lock"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let file = options.open(&path).context("cache publication locked")?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

struct Snapshot {
    bytes: Vec<u8>,
    modified: SystemTime,
    handle: same_file::Handle,
}
impl Snapshot {
    fn read(path: &Path) -> Result<Option<Self>> {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(meta) => {
                ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "cache must be a regular file"
                );
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    ensure!(
                        meta.file_attributes() & 0x400 == 0,
                        "cache reparse point rejected"
                    );
                }
            }
        }
        use std::io::Read;
        let file = fs::File::open(path)?;
        let modified = file.metadata()?.modified()?;
        let mut bytes = Vec::new();
        (&file).take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 64 * 1024 * 1024, "cache exceeds 64 MiB");
        ensure!(
            modified == file.metadata()?.modified()?,
            "cache changed during read"
        );
        Ok(Some(Self {
            bytes,
            modified,
            handle: same_file::Handle::from_file(file)?,
        }))
    }
}

fn publish(
    path: &Path,
    before: Option<Snapshot>,
    value: &Value,
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= 64 * 1024 * 1024, "cache exceeds 64 MiB");
    let mut temp = tempfile::Builder::new()
        .prefix(".wx-cache-")
        .tempfile_in(path.parent().context("cache parent required")?)?;
    temp.write_all(&bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    let now = Snapshot::read(path)?;
    match (&before, &now) {
        (None, None) => {
            before_commit()?;
            temp.persist_noclobber(path)
                .map_err(|e| e.error)
                .context("publish new cache")?;
        }
        (Some(a), Some(b)) => {
            ensure!(
                a.bytes == b.bytes && a.modified == b.modified && a.handle == b.handle,
                "cache changed; publication refused"
            );
            // Windows 替换前释放目标句柄；协作锁仍保持，但这不是非协作写者的 CAS。
            drop(now);
            drop(before);
            before_commit()?;
            temp.persist(path)
                .map_err(|e| e.error)
                .context("publish cache")?;
        }
        _ => anyhow::bail!("cache appeared or disappeared; publication refused"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
