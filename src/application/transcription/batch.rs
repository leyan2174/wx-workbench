//! 基于固定账号的数据库快照批量转录语音。
//! 成功缓存逐条提交，JSON 由既有回写层或 export_for 发布，不修改源数据库。
mod files;
use super::{
    cached::{self, CacheState, CachedRequest},
    receipt::ReceiptState,
    writeback::{self, VoiceIdentity, WritebackReport},
    Backend,
};
use crate::{
    adapters::wechat::media::voice::{self as database_media, DecryptedSource, MessageIdentity},
    daemon::cache::DbCache,
    runtime::RuntimeContext,
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};
use zeroize::Zeroize;

pub struct DatabaseMaterials(HashMap<String, String>);

impl DatabaseMaterials {
    pub fn new(keys: HashMap<String, String>) -> Self {
        Self(keys)
    }
}

impl std::fmt::Debug for DatabaseMaterials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DatabaseMaterials([REDACTED])")
    }
}

impl Drop for DatabaseMaterials {
    fn drop(&mut self) {
        for (mut source, mut key) in self.0.drain() {
            source.zeroize();
            key.zeroize();
        }
    }
}

pub type SnapshotLoader = fn(&RuntimeContext) -> Result<Snapshot>;

#[derive(Debug, Clone)]
pub struct CacheOptions {
    /// 仅允许账号目录下的单个 JSON 文件名，禁止覆盖任意外部路径。
    pub file_name: String,
}

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            file_name: "batch-transcriptions.json".into(),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub backend_kind: String,
    /// 推理后端状态说明，不计入 failed 或持久化故障 warnings。
    pub engine_warnings: Vec<String>,
    pub transcribed: usize,
    pub skipped_existing: usize,
    pub skipped_non_voice: usize,
    pub failed: usize,
    pub cache_hits: usize,
    pub cache_persisted: usize,
    pub errors: Vec<Failure>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub index: usize,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct Warning {
    pub username: String,
    pub source: String,
    pub local_id: i64,
    pub error: String,
}

impl Report {
    fn for_backend(backend: &Backend) -> Self {
        let mut report = Self {
            backend_kind: backend.identity().as_str().into(),
            ..Self::default()
        };
        if matches!(backend, Backend::PythonWhisper(_)) {
            let warning = "python_whisper uses local Python/PyTorch inference; Rust owns database association, cache and writeback. Named models retain Whisper's on-demand weight download behavior; audio is not uploaded.";
            eprintln!("[asr-batch] {warning}");
            report.engine_warnings.push(warning.into());
        }
        report
    }

    fn finish(mut self, report: WritebackReport) -> Self {
        self.transcribed = report.transcribed;
        self.skipped_existing = report.skipped_existing;
        self.skipped_non_voice = report.skipped_non_voice;
        self.failed = report.failed;
        self.errors = report
            .errors
            .into_iter()
            .map(|error| Failure {
                index: error.index,
                error: error.error,
            })
            .collect();
        for error in &self.errors {
            eprintln!(
                "[asr-batch] message index {} failed: {}",
                error.index, error.error
            );
        }
        self
    }
}

pub struct Snapshot {
    account_id: String,
    sources: Vec<DecryptedSource>,
    // 私有快照存活至批次对象释放，避免共享 daemon 缓存被并发更新。
    _directory: tempfile::TempDir,
}

impl Snapshot {
    /// 调用者须保持 Snapshot 存活；切勿只保存路径后释放所有者。
    pub fn sources(&self) -> &[DecryptedSource] {
        &self.sources
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }
}

pub struct BatchTranscriber {
    runtime: RuntimeContext,
    backend: Backend,
    cache_path: PathBuf,
    snapshot: Option<Snapshot>,
    snapshot_loader: SnapshotLoader,
}

impl BatchTranscriber {
    /// 不重新发现当前账号；构造阶段不访问音频、模型或云端。
    pub fn new(
        runtime: &RuntimeContext,
        backend: Backend,
        options: CacheOptions,
        snapshot_loader: SnapshotLoader,
    ) -> Result<Self> {
        backend.check_authorization()?;
        ensure!(
            !runtime.id.trim().is_empty(),
            "fixed account identity is required"
        );
        let name = &options.file_name;
        ensure!(
            name.ends_with(".json")
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                && !name.starts_with('.')
                && !name.contains(".."),
            "invalid ASR cache filename"
        );
        ensure!(
            runtime.directory.is_absolute(),
            "account directory must be absolute"
        );
        Ok(Self {
            runtime: runtime.clone(),
            backend,
            cache_path: runtime.directory.join(name),
            snapshot: None,
            snapshot_loader,
        })
    }

    /// 只填补没有 transcription 字段的语音，不过滤、排序或增加消息。
    /// 每条错误在报告和 stderr 可见；调用者应先发布成功文档再处理失败状态。
    pub fn process(&mut self, document: &mut Value) -> Result<Report> {
        self.backend.check_authorization()?;
        let mut report = Report::for_backend(&self.backend);
        let mut preparation_error = None;
        let written = writeback::transcribe_json(document, |identity| {
            self.transcribe(identity, &mut report, &mut preparation_error)
        })?;
        Ok(report.finish(written))
    }

    /// delta UID 依赖原始字段；只复制成功转录到 extras，绝不改动 raw_content。
    pub fn process_delta(&mut self, document: &mut Value) -> Result<Report> {
        self.backend.check_authorization()?;
        ensure!(
            document.get("source_error").is_none_or(Value::is_null),
            "delta source has an error; transcription refused"
        );
        let username = document
            .get("username")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .context("delta requires exact username")?;
        let messages = document
            .get("messages")
            .and_then(Value::as_array)
            .context("delta messages must be an array")?;
        let mut pending = Vec::with_capacity(messages.len());
        for message in messages {
            ensure!(message.is_object(), "delta message must be an object");
            if let Some(extras) = message.get("extras") {
                ensure!(extras.is_object(), "delta extras must be an object");
            }
            let mut normal = serde_json::json!({
                "source": message.get("db_path"), "local_id": message.get("local_id"),
                "timestamp": message.get("timestamp"), "type": message.get("msg_type"),
            });
            if let Some(value) = message.get("username") {
                normal["username"] = value.clone();
            }
            if let Some(value) = message
                .get("extras")
                .and_then(|extras| extras.get("transcription"))
            {
                normal["transcription"] = value.clone();
            }
            pending.push(normal);
        }
        let mut normal = serde_json::json!({"username": username, "messages": pending});
        let report = self.process(&mut normal)?;
        let messages = document["messages"]
            .as_array_mut()
            .expect("validated delta messages");
        for (message, processed) in messages
            .iter_mut()
            .zip(normal["messages"].as_array().expect("normal messages"))
        {
            if let Some(text) = processed.get("transcription") {
                let message = message.as_object_mut().expect("validated delta message");
                let extras = message
                    .entry("extras")
                    .or_insert_with(|| serde_json::json!({}));
                extras
                    .as_object_mut()
                    .expect("validated delta extras")
                    .entry("transcription")
                    .or_insert_with(|| text.clone());
            }
        }
        Ok(report)
    }

    /// 同身份旧输出仅提供已有转录，不将其消息集合合并进输入。
    pub fn process_file(&mut self, input: &Path, output: &Path) -> Result<Report> {
        self.backend.check_authorization()?;
        files::process_file(self, input, output)
    }

    fn protect_output(&self, output: &Path) -> Result<()> {
        crate::infrastructure::publication::validate_export_target(
            &self.runtime,
            &std::path::absolute(output)?,
        )?;
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let output = parent
            .canonicalize()?
            .join(output.file_name().context("output filename required")?);
        for path in [
            &self.runtime.directory,
            &self.runtime.config.db_dir,
            &self.runtime.config.decrypted_dir,
            &self.runtime.config.keys_file,
            &self.runtime.config_path,
        ] {
            if let Ok(protected) = path.canonicalize() {
                ensure!(
                    !output.starts_with(&protected),
                    "output overlaps account data or configuration"
                );
                if output.exists() && protected.is_file() {
                    ensure!(
                        !same_file::is_same_file(&output, &protected)?,
                        "output aliases protected file"
                    );
                }
            }
        }
        Ok(())
    }

    fn transcribe(
        &mut self,
        identity: &VoiceIdentity,
        report: &mut Report,
        preparation_error: &mut Option<String>,
    ) -> Result<String> {
        self.backend.check_authorization()?;
        if self.snapshot.is_none() && preparation_error.is_none() {
            match (self.snapshot_loader)(&self.runtime) {
                Ok(snapshot) => self.snapshot = Some(snapshot),
                Err(error) => {
                    *preparation_error = Some(format!("database snapshot unavailable: {error:#}"))
                }
            }
        }
        if let Some(error) = preparation_error {
            anyhow::bail!("{error}");
        }
        let snapshot = self.snapshot.as_ref().context("snapshot missing")?;
        let sources: Vec<_> = snapshot
            .sources()
            .iter()
            .filter(|source| {
                let name = source.source.replace('\\', "/").to_ascii_lowercase();
                name != "message/message_resource.db"
                    && !name.starts_with("message/message_resource_")
            })
            .cloned()
            .collect();
        let voice = database_media::resolve_voice_sources(
            &sources,
            MessageIdentity {
                username: &identity.username,
                source: &identity.source,
                local_id: identity.local_id,
            },
            None,
        )?;
        let evidence = &voice.evidence;
        // 以消息分片和消息 ID 建缓存；媒体 local_id 仅作为 receipt 证据。
        let request = CachedRequest {
            cache_path: &self.cache_path,
            account: &self.runtime.id,
            username: &evidence.username,
            source: &evidence.message_source,
            local_id: evidence.message_local_id,
            create_time: evidence.create_time,
            silk: &voice.silk,
        };
        let outcome = cached::transcribe_cached_with_receipt_checked(
            &request,
            evidence,
            &self.backend,
            |transcription| {
                ensure!(
                    !transcription.text.trim().is_empty(),
                    "transcription is empty"
                );
                Ok(())
            },
        )?;
        ensure!(
            !outcome.cached.transcription.text.trim().is_empty(),
            "cached transcription is empty"
        );
        match outcome.cached.cache_state {
            CacheState::Hit => report.cache_hits += 1,
            CacheState::Stored | CacheState::AlreadyPresent => report.cache_persisted += 1,
            state => warn(
                report,
                identity,
                format!(
                    "cache {state:?}; this result may require recognition again after interruption"
                ),
            ),
        }
        if !matches!(
            outcome.receipt,
            ReceiptState::Stored | ReceiptState::AlreadyPresent
        ) {
            warn(
                report,
                identity,
                format!(
                    "receipt {:?}; receipt-only recovery unavailable",
                    outcome.receipt
                ),
            );
        }
        eprintln!(
            "[asr-batch] {} {} {}: {:?}",
            identity.username, identity.source, identity.local_id, outcome.cached.cache_state
        );
        Ok(outcome.cached.transcription.text)
    }
}

fn warn(report: &mut Report, id: &VoiceIdentity, error: String) {
    eprintln!(
        "[asr-batch] {} {} {}: {}",
        id.username, id.source, id.local_id, error
    );
    report.warnings.push(Warning {
        username: id.username.clone(),
        source: id.source.clone(),
        local_id: id.local_id,
        error,
    });
}

#[derive(PartialEq, Eq)]
struct SourceState {
    path: PathBuf,
    identity: same_file::Handle,
    length: u64,
    modified: SystemTime,
}

fn states(paths: &[PathBuf]) -> Result<Vec<SourceState>> {
    let mut result = Vec::new();
    for path in paths {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let path = PathBuf::from(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() =>
                {
                    continue
                }
                Err(error) => return Err(error.into()),
            };
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "invalid database source"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "reparse database source"
                );
            }
            result.push(SourceState {
                identity: same_file::Handle::from_path(&path)?,
                path,
                length: metadata.len(),
                modified: metadata.modified()?,
            });
        }
    }
    Ok(result)
}

/// 账号级完整只读快照，不要求 ASR 后端，不创建转录缓存。
/// 私有解密产物由返回值持有，适用于 ASR 和聊天目录导出。
pub fn prepare_snapshot(
    runtime: &RuntimeContext,
    mut supplied: DatabaseMaterials,
) -> Result<Snapshot> {
    ensure!(
        !runtime.id.trim().is_empty() && runtime.directory.is_absolute(),
        "fixed account context required"
    );
    // 独立线程运行异步解密，允许同步回调由现有 Tokio 宿主调用。
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<Snapshot> {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                executor.block_on(async {
                    let paths = snapshot_source_files(&runtime.config.db_dir)?;
                    let before = states(&paths)?;
                    let root = runtime.config.db_dir.canonicalize()?;
                    let mut normalized = DatabaseMaterials::new(HashMap::new());
                    for (mut original_source, mut key) in supplied.0.drain() {
                        let source = original_source.replace('\\', "/").to_ascii_lowercase();
                        original_source.zeroize();
                        if source.starts_with("message/message_")
                            || source.starts_with("message/media_")
                            || source == "contact/contact.db"
                        {
                            ensure!(
                                normalized.0.insert(source, key).is_none(),
                                "duplicate database key source"
                            );
                        } else {
                            key.zeroize();
                        }
                    }
                    let mut selected = DatabaseMaterials::new(HashMap::new());
                    for path in &paths {
                        let source = path
                            .strip_prefix(&root)?
                            .to_str()
                            .context("non-UTF8 source")?
                            .replace('\\', "/");
                        let key = normalized
                            .0
                            .remove(&source.to_ascii_lowercase())
                            .context("database source has no account key")?;
                        selected.0.insert(source, key);
                    }
                    ensure!(
                        normalized.0.is_empty(),
                        "account key inventory contains missing database sources"
                    );
                    fs::create_dir_all(&runtime.directory)?;
                    let directory = tempfile::Builder::new()
                        .prefix("export-snapshot-")
                        .tempdir_in(&runtime.directory)?;
                    let db = DbCache::with_dirs(
                        root,
                        directory.path().to_owned(),
                        directory.path().join("_mtimes.json"),
                        std::mem::take(&mut selected.0),
                    )
                    .await?;
                    let mut sources = Vec::new();
                    for source in db.raw_db_keys() {
                        let path = db
                            .get(&source)
                            .await?
                            .context("database snapshot missing")?;
                        sources.push(DecryptedSource { source, path });
                    }
                    ensure!(
                        paths == snapshot_source_files(&runtime.config.db_dir)?
                            && before == states(&paths)?,
                        "account database changed while preparing snapshot; retry batch"
                    );
                    Ok(Snapshot {
                        account_id: runtime.id.clone(),
                        sources,
                        _directory: directory,
                    })
                })
            })
            .join()
            .map_err(|_| anyhow::anyhow!("database snapshot worker panicked"))?
    })
}

fn snapshot_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = database_media::source_files(root)?;
    let directory = root.canonicalize()?.join("message");
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        ensure!(index < 4096, "database directory entry limit exceeded");
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let resource = name == "message_resource.db"
            || name
                .strip_prefix("message_resource_")
                .and_then(|s| s.strip_suffix(".db"))
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
        if resource {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut identities = std::collections::HashSet::new();
    for path in &paths {
        ensure!(
            identities.insert(same_file::Handle::from_path(path)?),
            "snapshot sources alias the same file"
        );
    }
    // 与主库同样拒绝资源库重解析点；这一步不打开 SQLite，也不解密。
    let _ = states(&paths)?;
    Ok(paths)
}
