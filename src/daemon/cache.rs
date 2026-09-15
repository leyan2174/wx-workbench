use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[path = "cache/snapshot.rs"]
mod snapshot;
pub(crate) use snapshot::ResourceSnapshot;

use crate::crypto;
use crate::crypto::wal;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MtimeEntry {
    db_mt: u64,
    wal_mt: u64,
    path: String,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    db_mtime: u64,
    wal_mtime: u64,
    decrypted_path: PathBuf,
}

/// `DbCache::get_with_mode()` 本次解析 rel_key 时实际走了哪条路径。
///
/// 耗时由 `get_with_timing` 实测，不由模式推算：
/// - `CacheHit`：只返回已有解密产物
/// - `WalIncremental`：只在 cached DB 上增量应用 WAL
/// - `FullDecrypt`：全量解密，再按需应用 WAL
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Path 1：主 `.db` 和 WAL 都没变，直接命中缓存。
    CacheHit,
    /// Path 2：主 `.db` 没变、只有 WAL 变了，在 cached DB 上增量 apply。
    WalIncremental,
    /// Path 3：主 `.db` 变了或缓存 miss，重新 full decrypt。
    FullDecrypt,
}

impl CacheMode {
    /// 手工固定为 snake_case 字符串，避免未来给 enum 直接 derive `Serialize`
    /// 时静默改变 wire 形态。
    pub fn as_str(self) -> &'static str {
        match self {
            CacheMode::CacheHit => "cache_hit",
            CacheMode::WalIncremental => "wal_incremental",
            CacheMode::FullDecrypt => "full_decrypt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheResolve {
    pub path: PathBuf,
    pub mode: CacheMode,
}

/// 本次缓存解析的实测阶段；未执行的解密阶段为 None，不伪造零耗时。
#[derive(Debug, Clone)]
pub struct CacheTimings {
    pub resolve: Duration,
    pub db_decrypt: Option<Duration>,
    pub wal_apply: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct TimedCacheResolve {
    pub resolved: CacheResolve,
    pub timing: CacheTimings,
}

pub const LATENCY_PROBE_KIND: &str = "session_timestamp_probe_v1";

/// 固定 SessionTable 时间戳探测，不返回会话身份、正文、密钥或磁盘路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyProbe {
    pub version: u32,
    pub query_kind: String,
    pub cache_mode: String,
    /// 含缓存锁等待、工作线程排队和索引更新，不能当作纯解密耗时。
    pub daemon_cache_resolve_ms: f64,
    /// 仅合计实际执行的 DB 解密与 WAL 应用，不从端到端耗时相减。
    pub daemon_decrypt_ms: Option<f64>,
    pub daemon_db_decrypt_ms: Option<f64>,
    pub daemon_wal_apply_ms: Option<f64>,
    /// 在线程内部计时，包含只读连接建立、固定 SQL 遍历及连接关闭，不含排队。
    pub daemon_query_ms: f64,
    pub decrypt_skipped_reason: Option<String>,
    pub rows_read: usize,
    pub row_limit: usize,
    pub possible_truncation: bool,
    pub latest_timestamp: Option<i64>,
}

impl LatencyProbe {
    /// 客户端校验测量契约，拒绝缺字段、非有限数或把未执行阶段写成零的响应。
    pub fn validate(&self, expected_limit: usize) -> Result<()> {
        ensure!(
            self.version == 1 && self.query_kind == LATENCY_PROBE_KIND,
            "后台延迟探测协议版本或查询类型不符"
        );
        ensure!(
            (1..=10_000).contains(&self.row_limit)
                && self.row_limit == expected_limit
                && self.rows_read <= self.row_limit
                && self.possible_truncation == (self.rows_read == self.row_limit),
            "后台延迟探测行数或上限无效"
        );
        ensure!(
            if self.rows_read == 0 {
                self.latest_timestamp.is_none()
            } else {
                self.latest_timestamp.is_some_and(|value| value > 0)
            },
            "后台延迟探测时间戳与行数不一致"
        );
        for value in [
            Some(self.daemon_cache_resolve_ms),
            self.daemon_decrypt_ms,
            self.daemon_db_decrypt_ms,
            self.daemon_wal_apply_ms,
            Some(self.daemon_query_ms),
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                value.is_finite() && value >= 0.0,
                "后台延迟探测耗时不是有效的非负有限数"
            );
        }
        let expected_decrypt = sum_measured(self.daemon_db_decrypt_ms, self.daemon_wal_apply_ms);
        ensure!(
            match (expected_decrypt, self.daemon_decrypt_ms) {
                (None, None) => true,
                (Some(expected), Some(actual)) =>
                    (expected - actual).abs() <= 1e-6 * expected.max(1.0),
                _ => false,
            },
            "后台解密总耗时与实测分项不一致"
        );
        if let Some(decrypt) = self.daemon_decrypt_ms {
            ensure!(
                decrypt <= self.daemon_cache_resolve_ms + 1e-6 * decrypt.max(1.0),
                "后台解密耗时超出缓存解析边界"
            );
        }
        let expected_skip = match self.cache_mode.as_str() {
            "cache_hit" => {
                ensure!(
                    self.daemon_db_decrypt_ms.is_none() && self.daemon_wal_apply_ms.is_none(),
                    "缓存命中不应报告已执行解密"
                );
                Some("cache_hit_no_decryption")
            }
            "full_decrypt" => {
                ensure!(
                    self.daemon_db_decrypt_ms.is_some(),
                    "全量解密缺少实际 DB 阶段耗时"
                );
                None
            }
            "wal_incremental" => {
                ensure!(
                    self.daemon_db_decrypt_ms.is_none(),
                    "WAL 增量不应报告全量 DB 解密"
                );
                self.daemon_wal_apply_ms
                    .is_none()
                    .then_some("wal_absent_no_decryption")
            }
            _ => anyhow::bail!("后台延迟探测缓存模式无效"),
        };
        ensure!(
            self.decrypt_skipped_reason.as_deref() == expected_skip,
            "后台解密跳过原因与缓存模式不符"
        );
        Ok(())
    }
}

fn sum_measured(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first + second),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// 解密后数据库的 mtime-aware 缓存
///
/// 当数据库文件（.db）或 WAL 文件（.db-wal）的 mtime 发生变化时，
/// 自动重新解密并更新缓存。跨进程重启可通过持久化 mtime 文件复用已解密的 DB。
pub struct DbCache {
    db_dir: PathBuf,
    cache_dir: PathBuf,
    mtime_file: PathBuf,
    output_protected_paths: Vec<PathBuf>,
    all_keys: HashMap<String, String>, // rel_key -> enc_key(hex)
    inner: Arc<Mutex<HashMap<String, CacheEntry>>>,
    work: Arc<Mutex<()>>,
    #[cfg(test)]
    before_commit: CommitHook,
}

impl Drop for DbCache {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.all_keys.values_mut().for_each(Zeroize::zeroize);
    }
}

#[cfg(test)]
type CommitHook = Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

impl DbCache {
    /// Use explicit account paths; query generations additionally share a work lock.
    pub(crate) async fn with_dirs(
        db_dir: PathBuf,
        cache_dir: PathBuf,
        mtime_file: PathBuf,
        all_keys: HashMap<String, String>,
    ) -> Result<Self> {
        Self::with_dirs_coordinated(
            db_dir,
            cache_dir,
            mtime_file,
            all_keys,
            Arc::new(Mutex::new(())),
        )
        .await
    }

    pub(crate) async fn with_dirs_coordinated(
        db_dir: PathBuf,
        cache_dir: PathBuf,
        mtime_file: PathBuf,
        all_keys: HashMap<String, String>,
        work: Arc<Mutex<()>>,
    ) -> Result<Self> {
        // 初始化请求取消后，旧代际仍可能有尚未完成的阻塞提交；先等它登记完毕。
        let _pending = work.lock().await;
        tokio::fs::create_dir_all(&cache_dir).await?;

        let cache = DbCache {
            db_dir,
            cache_dir,
            mtime_file,
            output_protected_paths: Vec::new(),
            all_keys,
            inner: Arc::new(Mutex::new(HashMap::new())),
            work: work.clone(),
            #[cfg(test)]
            before_commit: Default::default(),
        };

        cache.load_persistent().await;
        Ok(cache)
    }

    /// 数据库根目录（即 `<wxchat_base>/db_storage`）。
    /// 上层（attachment resolver）需要 `db_dir.parent()` 来定位 `msg/attach/...` 解密图片。
    pub fn db_dir(&self) -> &Path {
        &self.db_dir
    }

    #[cfg(test)]
    pub(super) fn set_before_commit(&self, hook: impl FnOnce() + Send + 'static) {
        *self.before_commit.lock().unwrap() = Some(Box::new(hook));
    }

    /// 返回固定账号的输出保护路径，不读取密钥内容，也不刷新或解密缓存。
    pub(crate) fn output_protection_paths(&self) -> Result<Vec<PathBuf>> {
        let inner = self
            .inner
            .try_lock()
            .context("cache protection metadata is busy")?;
        let mut paths = vec![
            self.db_dir.clone(),
            self.cache_dir.clone(),
            self.mtime_file.clone(),
        ];
        paths.extend(self.output_protected_paths.iter().cloned());
        // 持久缓存可能记录缓存根之外的现有产物，也必须纳入保护。
        paths.extend(inner.values().map(|entry| entry.decrypted_path.clone()));
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// 只读枚举原始数据库键名，不返回密钥值；调用 get 时不得改写这些键。
    pub(crate) fn raw_db_keys(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.all_keys.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 仅规范化匹配，返回原始键供 get 精确查找；不暴露值或跳过缺失片。
    pub(crate) fn media_db_keys(&self) -> Vec<String> {
        self.raw_db_keys()
            .into_iter()
            .filter(|key| {
                let normalized = key.replace('\\', "/").to_ascii_lowercase();
                normalized
                    .strip_prefix("message/media_")
                    .and_then(|s| s.strip_suffix(".db"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .collect()
    }

    /// 丢弃指定数据库的缓存记录，使下次访问重新完整解密。
    pub async fn invalidate(&self, rel_key: &str) -> bool {
        let work = self.work.clone().lock_owned().await;
        let inner = self.inner.clone();
        let mtime_file = self.mtime_file.clone();
        let rel_key = rel_key.to_owned();
        tokio::task::spawn_blocking(move || {
            let _work = work;
            let mut inner = inner.blocking_lock();
            let mut next = inner.clone();
            let Some(removed) = next.remove(&rel_key) else {
                return false;
            };
            if persist_entries(&mtime_file, &next, &removed.decrypted_path).is_err() {
                eprintln!("[cache] Failed to persist cache invalidation");
                return false;
            }
            *inner = next;
            true
        })
        .await
        .unwrap_or(false)
    }

    fn cache_file_path(&self, rel_key: &str) -> PathBuf {
        let hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        self.cache_dir.join(format!("{}.db", hash))
    }

    /// 从持久化文件加载 mtime 记录，复用未过期的解密文件
    async fn load_persistent(&self) {
        let mtime_file = &self.mtime_file;
        let content = match tokio::fs::read_to_string(&mtime_file).await {
            Ok(c) => c,
            Err(_) => return,
        };
        let saved: HashMap<String, MtimeEntry> = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return,
        };

        let mut inner = self.inner.lock().await;
        let mut reused = 0usize;
        for (rel_key, entry) in &saved {
            let dec_path = PathBuf::from(&entry.path);
            if !dec_path.exists() {
                continue;
            }
            let db_path = self
                .db_dir
                .join(rel_key.replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR));
            let wal_path = wal_path_for(&db_path);

            let db_mt = mtime_nanos(&db_path);
            let _wal_mt = if wal_path.exists() {
                mtime_nanos(&wal_path)
            } else {
                0
            };

            // 只要主 .db 没变，就把 cached 产物载回来。
            // 如果 WAL mtime 变了，后续 `get()` 会自动走 Path 2：在已有 cached DB 上增量 apply_wal，
            // 而不是 daemon 重启后第一条请求又退回全量解密。
            if db_mt == entry.db_mt {
                inner.insert(
                    rel_key.clone(),
                    CacheEntry {
                        db_mtime: db_mt,
                        // 保留"cached 产物构建时看到的 wal_mtime"，让 `get()` 去比较当前 WAL
                        // 是否发生了变化，从而决定 exact-hit 还是 WAL 增量。
                        wal_mtime: entry.wal_mt,
                        decrypted_path: dec_path,
                    },
                );
                reused += 1;
            }
        }
        if reused > 0 {
            eprintln!("[cache] 复用 {} 个已解密 DB", reused);
        }
    }

    /// 获取解密后的数据库路径
    ///
    /// 三种命中路径：
    /// 1. 主 `.db` 和 WAL mtime 都未变 → 直接返回缓存路径
    /// 2. 主 `.db` 未变、WAL mtime 变了 → 在已有 cached 产物上**增量** `apply_wal`
    ///    （apply_wal 是幂等的：旧帧 redo 同样的 page 写入，新帧追加生效；不重新 full_decrypt）
    /// 3. 主 `.db` mtime 变了 → 重新 `full_decrypt` + `apply_wal`
    ///
    /// 主库未变且 WAL 更新时复用增量路径；实际耗时取决于数据量、构建模式与存储环境。
    pub async fn get(&self, rel_key: &str) -> Result<Option<PathBuf>> {
        Ok(self.get_with_mode(rel_key).await?.map(|r| r.path))
    }

    pub async fn get_with_mode(&self, rel_key: &str) -> Result<Option<CacheResolve>> {
        Ok(self
            .get_with_timing(rel_key)
            .await?
            .map(|result| result.resolved))
    }

    /// 与普通缓存请求共用三条解析路径；只增加本次调用的阶段计时，不维护全局计时状态。
    pub async fn get_with_timing(&self, rel_key: &str) -> Result<Option<TimedCacheResolve>> {
        let resolve_started = Instant::now();
        let enc_key_hex = match self.all_keys.get(rel_key) {
            Some(k) => k.clone(),
            None => return Ok(None),
        };
        let work = self.work.clone().lock_owned().await;
        let db_path = self
            .db_dir
            .join(rel_key.replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR));
        let wal_path = wal_path_for(&db_path);
        let out_path = self.cache_file_path(rel_key);
        let rel_key = rel_key.to_owned();
        let inner = self.inner.clone();
        let mtime_file = self.mtime_file.clone();
        #[cfg(test)]
        let before_commit = self.before_commit.clone();

        // 阻塞工作开始后持锁完成文件发布与索引登记，调用方取消只放弃接收结果。
        // 失效操作与新代际初始化必须等待这次提交，不能在发布与登记之间插入。
        tokio::task::spawn_blocking(move || {
            let _work = work;
            if !db_path.try_exists()? {
                return Ok(None);
            }
            let key = zeroize::Zeroizing::new(
                hex_to_32bytes(&enc_key_hex)
                    .with_context(|| format!("密钥格式错误: {}", rel_key))?,
            );
            let db_mt = mtime_nanos(&db_path);
            let wal_mt = mtime_nanos(&wal_path);
            let cached = inner.blocking_lock().get(&rel_key).cloned();
            let mut first_page = [0u8; crypto::PAGE_SZ];
            let mut source = std::fs::File::open(&db_path)?;
            std::io::Read::read_exact(&mut source, &mut first_page)?;
            ensure!(
                crypto::verify_page1(&key, &first_page),
                "数据库密钥已失效或源库头损坏，请重新初始化当前账号；现有缓存未改动"
            );
            drop(source);

            let mut out_path = out_path;
            let mut mode = CacheMode::FullDecrypt;
            if let Some(entry) = cached {
                if entry.db_mtime == db_mt && valid_cache_header(&entry.decrypted_path)? {
                    out_path = entry.decrypted_path;
                    if entry.wal_mtime == wal_mt {
                        return Ok(Some(TimedCacheResolve {
                            resolved: CacheResolve {
                                path: out_path,
                                mode: CacheMode::CacheHit,
                            },
                            timing: CacheTimings {
                                resolve: resolve_started.elapsed(),
                                db_decrypt: None,
                                wal_apply: None,
                            },
                        }));
                    }
                    mode = CacheMode::WalIncremental;
                }
            }

            let has_wal = wal_path.try_exists()?;
            let mut sources = vec![db_path.as_path()];
            if has_wal {
                sources.push(wal_path.as_path());
            }
            let (db_decrypt, wal_apply) = if mode == CacheMode::FullDecrypt {
                crypto::with_staged_output(&out_path, &sources, |temporary| {
                    let phase = Instant::now();
                    crypto::full_decrypt(&db_path, temporary, &key)?;
                    let db_decrypt = phase.elapsed();
                    let wal_apply = if has_wal {
                        let phase = Instant::now();
                        wal::apply_wal(&wal_path, temporary, &key, &db_path)?;
                        Some(phase.elapsed())
                    } else {
                        None
                    };
                    ensure!(
                        mtime_nanos(&db_path) == db_mt && mtime_nanos(&wal_path) == wal_mt,
                        "解密期间 DB 或 WAL 已变化，未发布缓存及索引"
                    );
                    Ok((Some(db_decrypt), wal_apply))
                })?
            } else if has_wal {
                let phase = Instant::now();
                wal::apply_wal(&wal_path, &out_path, &key, &db_path)?;
                (None, Some(phase.elapsed()))
            } else {
                (None, None)
            };
            #[cfg(test)]
            if let Some(hook) = before_commit.lock().unwrap().take() {
                hook();
            }
            let mut entries = inner.blocking_lock();
            let mut next = entries.clone();
            next.insert(
                rel_key.clone(),
                CacheEntry {
                    db_mtime: db_mt,
                    wal_mtime: wal_mt,
                    decrypted_path: out_path.clone(),
                },
            );
            persist_entries(&mtime_file, &next, &db_path)?;
            *entries = next;
            eprintln!(
                "[cache] {} {} ({}ms)",
                mode.as_str(),
                rel_key,
                resolve_started.elapsed().as_millis()
            );
            Ok(Some(TimedCacheResolve {
                resolved: CacheResolve {
                    path: out_path,
                    mode,
                },
                timing: CacheTimings {
                    resolve: resolve_started.elapsed(),
                    db_decrypt,
                    wal_apply,
                },
            }))
        })
        .await?
    }

    /// 延迟专用探测：复用真实缓存加载，再查询固定数据库；不预热、不强制失效、不接受任意 SQL。
    pub async fn latency_probe(&self, limit: usize) -> Result<LatencyProbe> {
        ensure!(
            (1..=10_000).contains(&limit),
            "延迟探测行数上限须在 1..10000 内"
        );
        let resolved = self
            .get_with_timing(crate::adapters::wechat::messages::probe::source_key())
            .await?
            .context("延迟探测无法加载当前账号 session.db")?;
        let path = resolved.resolved.path;
        let (rows_read, latest_timestamp, query_duration) =
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let observation = crate::adapters::wechat::messages::probe::observe(&path, limit)?;
                Ok::<_, anyhow::Error>((
                    observation.rows_read,
                    observation.latest_timestamp,
                    started.elapsed(),
                ))
            })
            .await??;
        let db_decrypt = resolved.timing.db_decrypt.map(milliseconds);
        let wal_apply = resolved.timing.wal_apply.map(milliseconds);
        let decrypt_skipped_reason = match resolved.resolved.mode {
            CacheMode::CacheHit => Some("cache_hit_no_decryption".into()),
            CacheMode::WalIncremental if wal_apply.is_none() => {
                Some("wal_absent_no_decryption".into())
            }
            _ => None,
        };
        let probe = LatencyProbe {
            version: 1,
            query_kind: LATENCY_PROBE_KIND.into(),
            cache_mode: resolved.resolved.mode.as_str().into(),
            daemon_cache_resolve_ms: milliseconds(resolved.timing.resolve),
            daemon_decrypt_ms: sum_measured(db_decrypt, wal_apply),
            daemon_db_decrypt_ms: db_decrypt,
            daemon_wal_apply_ms: wal_apply,
            daemon_query_ms: milliseconds(query_duration),
            decrypt_skipped_reason,
            rows_read,
            row_limit: limit,
            possible_truncation: rows_read == limit,
            latest_timestamp,
        };
        probe.validate(limit)?;
        Ok(probe)
    }
}

fn persist_entries(
    path: &Path,
    entries: &HashMap<String, CacheEntry>,
    source: &Path,
) -> Result<()> {
    let data: HashMap<_, _> = entries
        .iter()
        .map(|(key, entry)| {
            (
                key,
                MtimeEntry {
                    db_mt: entry.db_mtime,
                    wal_mt: entry.wal_mtime,
                    path: entry.decrypted_path.to_string_lossy().into_owned(),
                },
            )
        })
        .collect();
    let json = serde_json::to_vec_pretty(&data)?;
    let mut protected: Vec<_> = entries
        .values()
        .map(|entry| entry.decrypted_path.as_path())
        .filter(|path| path.exists())
        .collect();
    if source.exists() {
        protected.push(source);
    }
    crypto::with_staged_output(path, &protected, |temporary| {
        std::fs::write(temporary, &json).context("Failed to persist authenticated cache index")
    })
}

pub(super) fn mtime_nanos(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        })
        .unwrap_or(0)
}

/// `foo/bar.db` → `foo/bar.db-wal`（用 OsString 拼接，避免 display() 的 UTF-8 问题）
fn wal_path_for(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push("-wal");
    db_path.with_file_name(name)
}

/// 旧版本可能在密钥失效后写入无效缓存；不能仅凭源文件时间戳继续复用。
fn valid_cache_header(path: &Path) -> Result<bool> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let size = file.metadata()?.len();
    if size < crypto::PAGE_SZ as u64 || !size.is_multiple_of(crypto::PAGE_SZ as u64) {
        return Ok(false);
    }
    let mut header = [0u8; 100];
    std::io::Read::read_exact(&mut file, &mut header)?;
    Ok(header[..16] == *crypto::SQLITE_HDR
        && u16::from_be_bytes([header[16], header[17]]) as usize == crypto::PAGE_SZ
        && matches!(header[18], 1 | 2)
        && matches!(header[19], 1 | 2)
        && header[20] as usize == crypto::RESERVE_SZ)
}

fn hex_to_32bytes(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64 {
        anyhow::bail!("密钥 hex 长度应为 64，实际为 {}", s.len());
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("非法 hex 字符 at {}", i * 2))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成 SQLite 使用真实保留区和 AES-CBC 页格式；仅测试加密，生产仍复用 crypto。
    fn latency_encrypted_pages(
        path: &Path,
        timestamps: &[i64],
        key: &[u8; 32],
        wal_page: bool,
    ) -> Vec<u8> {
        use cbc::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
        use hmac::{Hmac, Mac};
        use sha2::Sha512;

        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA page_size=4096;").unwrap();
        let mut reserve: i32 = crypto::RESERVE_SZ as i32;
        // 连接及可写参数在调用期间有效，类型遵循 SQLite 的文件控制接口约定。
        let result = unsafe {
            rusqlite::ffi::sqlite3_file_control(
                conn.handle(),
                std::ptr::null(),
                rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
                (&mut reserve as *mut i32).cast(),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        conn.execute_batch("CREATE TABLE SessionTable(username TEXT PRIMARY KEY, last_timestamp INTEGER NOT NULL);").unwrap();
        for (index, timestamp) in timestamps.iter().enumerate() {
            conn.execute(
                "INSERT INTO SessionTable VALUES(?1,?2)",
                rusqlite::params![format!("synthetic-user-{index}"), timestamp],
            )
            .unwrap();
        }
        drop(conn);
        let plain = std::fs::read(path).unwrap();
        assert_eq!(plain[20] as usize, crypto::RESERVE_SZ);
        assert_eq!(plain.len() % crypto::PAGE_SZ, 0);
        let salt = [0x35u8; 16];
        let mac_salt = salt.map(|byte| byte ^ 0x3a);
        let mut mac_key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha512>(key, &mac_salt, 2, &mut mac_key);
        let mut encrypted = Vec::new();
        for (index, page) in plain.chunks_exact(crypto::PAGE_SZ).enumerate() {
            let start = if index == 0 && !wal_page {
                crypto::SALT_SZ
            } else {
                0
            };
            let iv = [0x24; 16];
            let cipher = cbc::Encryptor::<aes::Aes256>::new(key.into(), (&iv).into())
                .encrypt_padded_vec_mut::<NoPadding>(&page[start..4016]);
            let mut result = vec![0u8; crypto::PAGE_SZ];
            if start != 0 {
                result[..16].copy_from_slice(&salt);
            }
            result[start..4016].copy_from_slice(&cipher);
            result[4016..4032].copy_from_slice(&iv);
            let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
            mac.update(&result[start..4032]);
            mac.update(&((index + 1) as u32).to_le_bytes());
            result[4032..].copy_from_slice(&mac.finalize().into_bytes());
            encrypted.extend(result);
        }
        if !wal_page {
            assert!(crypto::verify_page1(key, &encrypted[..crypto::PAGE_SZ]));
        }
        encrypted
    }

    use crate::crypto::test_support::wal_bytes as latency_wal_bytes;

    async fn latency_fixture(
        temp: &tempfile::TempDir,
        timestamps: &[i64],
    ) -> (DbCache, PathBuf, Vec<u8>) {
        let source = temp.path().join("source/session/session.db");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let encrypted = latency_encrypted_pages(
            &temp.path().join("plain.db"),
            timestamps,
            &[0x42; 32],
            false,
        );
        std::fs::write(&source, &encrypted).unwrap();
        let cache_dir = temp.path().join("cache");
        let cache = DbCache::with_dirs(
            temp.path().join("source"),
            cache_dir.clone(),
            cache_dir.join("_mtimes.json"),
            HashMap::from([("session/session.db".into(), "42".repeat(32))]),
        )
        .await
        .unwrap();
        (cache, source, encrypted)
    }

    #[tokio::test]
    async fn cancelled_full_and_wal_commits_finish_once_and_restart_as_hits() {
        use std::{future::Future, task::Poll};
        for incremental in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (cache, source, original) = latency_fixture(&temp, &[100]).await;
            let cache = Arc::new(cache);
            let before = if incremental {
                cache.get("session/session.db").await.unwrap().unwrap();
                let pages = latency_encrypted_pages(
                    &temp.path().join("wal-plain.db"),
                    &[200],
                    &[0x42; 32],
                    true,
                );
                std::fs::write(wal_path_for(&source), latency_wal_bytes(&pages)).unwrap();
                Some(std::fs::read(&cache.mtime_file).unwrap())
            } else {
                None
            };
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            cache.set_before_commit(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            });
            let first_cache = cache.clone();
            let first = tokio::spawn(async move { first_cache.get("session/session.db").await });
            tokio::time::timeout(Duration::from_secs(10), entered_rx)
                .await
                .unwrap()
                .unwrap();
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            assert!(valid_cache_header(&cache.cache_file_path("session/session.db")).unwrap());
            if let Some(before) = before {
                assert_eq!(std::fs::read(&cache.mtime_file).unwrap(), before);
                assert_eq!(cache.inner.lock().await["session/session.db"].wal_mtime, 0);
            } else {
                assert!(!cache.mtime_file.exists());
                assert!(cache.inner.lock().await.is_empty());
            }
            let mut next = Box::pin(cache.get_with_mode("session/session.db"));
            std::future::poll_fn(|cx| {
                assert!(matches!(next.as_mut().poll(cx), Poll::Pending));
                Poll::Ready(())
            })
            .await;
            release_tx.send(()).unwrap();
            let next = tokio::time::timeout(Duration::from_secs(10), next)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(next.mode, CacheMode::CacheHit);
            let conn = rusqlite::Connection::open(&next.path).unwrap();
            let timestamp: i64 = conn
                .query_row("SELECT last_timestamp FROM SessionTable", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(timestamp, if incremental { 200 } else { 100 });
            drop(conn);
            let restart = DbCache::with_dirs(
                cache.db_dir.clone(),
                cache.cache_dir.clone(),
                cache.mtime_file.clone(),
                cache.all_keys.clone(),
            )
            .await
            .unwrap();
            assert_eq!(
                restart
                    .get_with_mode("session/session.db")
                    .await
                    .unwrap()
                    .unwrap()
                    .mode,
                CacheMode::CacheHit
            );
            assert_eq!(std::fs::read(source).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn cancelled_generation_commit_precedes_reinitialization_and_invalidation() {
        use std::{future::Future, task::Poll};
        for invalidate in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (cache, _, _) = latency_fixture(&temp, &[100]).await;
            let cache = Arc::new(cache);
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            cache.set_before_commit(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            });
            let first_cache = cache.clone();
            let first = tokio::spawn(async move { first_cache.get("session/session.db").await });
            tokio::time::timeout(Duration::from_secs(10), entered_rx)
                .await
                .unwrap()
                .unwrap();
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            if invalidate {
                let mut removal = Box::pin(cache.invalidate("session/session.db"));
                std::future::poll_fn(|cx| {
                    assert!(matches!(removal.as_mut().poll(cx), Poll::Pending));
                    Poll::Ready(())
                })
                .await;
                release_tx.send(()).unwrap();
                assert!(tokio::time::timeout(Duration::from_secs(10), removal)
                    .await
                    .unwrap());
                assert!(cache.inner.lock().await.is_empty());
                let index: HashMap<String, MtimeEntry> =
                    serde_json::from_slice(&std::fs::read(&cache.mtime_file).unwrap()).unwrap();
                assert!(index.is_empty());
            } else {
                let db_dir = cache.db_dir.clone();
                let cache_dir = cache.cache_dir.clone();
                let index = cache.mtime_file.clone();
                let keys = cache.all_keys.clone();
                let work = cache.work.clone();
                drop(cache);
                let mut restart = Box::pin(DbCache::with_dirs_coordinated(
                    db_dir, cache_dir, index, keys, work,
                ));
                std::future::poll_fn(|cx| {
                    assert!(matches!(restart.as_mut().poll(cx), Poll::Pending));
                    Poll::Ready(())
                })
                .await;
                release_tx.send(()).unwrap();
                let restart = tokio::time::timeout(Duration::from_secs(10), restart)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    restart
                        .get_with_mode("session/session.db")
                        .await
                        .unwrap()
                        .unwrap()
                        .mode,
                    CacheMode::CacheHit
                );
            }
        }
    }

    #[tokio::test]
    async fn index_publication_failure_does_not_register_an_uncommitted_entry() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, _, _) = latency_fixture(&temp, &[100]).await;
        std::fs::create_dir(&cache.mtime_file).unwrap();
        assert!(cache.get("session/session.db").await.is_err());
        assert!(cache.inner.lock().await.is_empty());
        assert!(cache.mtime_file.is_dir());
        assert!(valid_cache_header(&cache.cache_file_path("session/session.db")).unwrap());
        std::fs::remove_dir(&cache.mtime_file).unwrap();
        assert_eq!(
            cache
                .get_with_mode("session/session.db")
                .await
                .unwrap()
                .unwrap()
                .mode,
            CacheMode::FullDecrypt
        );
    }

    #[tokio::test]
    async fn failed_wal_preserves_cache_and_index_for_full_and_incremental_paths() {
        for (changed_source, bad_header) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let temp = tempfile::tempdir().unwrap();
            let (cache, source, _) = latency_fixture(&temp, &[100]).await;
            let output = cache.get("session/session.db").await.unwrap().unwrap();
            let cached = std::fs::read(&output).unwrap();
            let index = std::fs::read(&cache.mtime_file).unwrap();
            let old_entry = cache.inner.lock().await["session/session.db"].clone();
            if changed_source {
                let old_time = std::fs::metadata(&source).unwrap().modified().unwrap();
                let replacement = latency_encrypted_pages(
                    &temp.path().join("replacement.db"),
                    &[200],
                    &[0x42; 32],
                    false,
                );
                std::fs::write(&source, replacement).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&source)
                    .unwrap()
                    .set_times(
                        std::fs::FileTimes::new().set_modified(old_time + Duration::from_secs(1)),
                    )
                    .unwrap();
            }
            let source_before = std::fs::read(&source).unwrap();
            let mut pages =
                latency_encrypted_pages(&temp.path().join("bad-wal.db"), &[300], &[0x42; 32], true);
            // 重算 WAL 校验和而不重签页面，确保错误来自认证而非未提交尾部。
            pages[100] ^= 1;
            let mut wal = latency_wal_bytes(&pages);
            if bad_header {
                wal[24] ^= 1;
            }
            let wal_path = wal_path_for(&source);
            std::fs::write(&wal_path, &wal).unwrap();
            assert!(cache.get("session/session.db").await.is_err());
            assert_eq!(std::fs::read(&output).unwrap(), cached);
            assert_eq!(std::fs::read(&cache.mtime_file).unwrap(), index);
            assert_eq!(std::fs::read(&source).unwrap(), source_before);
            assert_eq!(std::fs::read(&wal_path).unwrap(), wal);
            assert_eq!(cache.inner.lock().await["session/session.db"].wal_mtime, 0);
            assert_eq!(
                cache.inner.lock().await["session/session.db"].db_mtime,
                old_entry.db_mtime
            );
        }
    }

    #[tokio::test]
    async fn invalid_cache_header_is_rebuilt_from_authenticated_source() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, source, encrypted) = latency_fixture(&temp, &[100]).await;
        let output = cache.get("session/session.db").await.unwrap().unwrap();
        std::fs::write(&output, vec![0x91; crypto::PAGE_SZ]).unwrap();
        let recovered = cache
            .get_with_mode("session/session.db")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.mode, CacheMode::FullDecrypt);
        assert!(valid_cache_header(&output).unwrap());
        assert_eq!(std::fs::read(source).unwrap(), encrypted);
    }

    #[tokio::test]
    async fn stale_key_cannot_overwrite_existing_cache_or_index() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, source, _) = latency_fixture(&temp, &[100]).await;
        let output = cache.get("session/session.db").await.unwrap().unwrap();
        let cached = std::fs::read(&output).unwrap();
        let index = std::fs::read(&cache.mtime_file).unwrap();
        let replacement = latency_encrypted_pages(
            &temp.path().join("replacement.db"),
            &[200],
            &[0x24; 32],
            false,
        );
        std::fs::write(&source, replacement).unwrap();
        let error = cache.get("session/session.db").await.unwrap_err();
        assert!(error.to_string().contains("现有缓存未改动"));
        assert_eq!(std::fs::read(output).unwrap(), cached);
        assert_eq!(std::fs::read(&cache.mtime_file).unwrap(), index);
    }

    #[tokio::test]
    async fn latency_probe_measures_real_encrypted_cold_and_warm_cache() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, source, encrypted) = latency_fixture(&temp, &[100, 200, 0]).await;
        let cold = cache.latency_probe(10).await.unwrap();
        cold.validate(10).unwrap();
        assert_eq!(cold.cache_mode, "full_decrypt");
        assert!(cold.daemon_db_decrypt_ms.is_some());
        assert_eq!(cold.daemon_decrypt_ms, cold.daemon_db_decrypt_ms);
        assert!(cold.daemon_wal_apply_ms.is_none());
        assert_eq!((cold.rows_read, cold.latest_timestamp), (2, Some(200)));
        assert!(!cold.possible_truncation);
        assert_eq!(std::fs::read(&source).unwrap(), encrypted);
        let output = cache.cache_file_path("session/session.db");
        let cached = std::fs::read(&output).unwrap();
        let modified = std::fs::metadata(&output).unwrap().modified().unwrap();
        assert!(cached.starts_with(crypto::SQLITE_HDR));
        assert!(!encrypted.starts_with(crypto::SQLITE_HDR));

        // 热缓存仍校验源库第一页；用缓存内容、mtime 和计时字段确认没有重新解密。
        let warm = cache.latency_probe(1).await.unwrap();
        warm.validate(1).unwrap();
        assert_eq!(warm.cache_mode, "cache_hit");
        assert!(warm.daemon_decrypt_ms.is_none());
        assert!(warm.daemon_db_decrypt_ms.is_none());
        assert!(warm.daemon_wal_apply_ms.is_none());
        assert_eq!(
            warm.decrypt_skipped_reason.as_deref(),
            Some("cache_hit_no_decryption")
        );
        assert_eq!((warm.rows_read, warm.latest_timestamp), (1, Some(200)));
        assert!(warm.possible_truncation);
        assert!(warm.daemon_cache_resolve_ms.is_finite() && warm.daemon_query_ms.is_finite());
        assert_eq!(std::fs::read(&output).unwrap(), cached);
        assert_eq!(
            std::fs::metadata(&output).unwrap().modified().unwrap(),
            modified
        );
        assert!(!output.with_extension("db-journal").exists());
    }

    #[tokio::test]
    async fn latency_probe_measures_full_and_incremental_wal_without_inventing_skipped_time() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, source, encrypted) = latency_fixture(&temp, &[100]).await;
        let wal_path = wal_path_for(&source);
        let first = latency_wal_bytes(&latency_encrypted_pages(
            &temp.path().join("wal-first.db"),
            &[300, 400],
            &[0x42; 32],
            true,
        ));
        std::fs::write(&wal_path, &first).unwrap();
        let cold = cache.latency_probe(10).await.unwrap();
        cold.validate(10).unwrap();
        assert_eq!(cold.cache_mode, "full_decrypt");
        assert!(cold.daemon_db_decrypt_ms.is_some() && cold.daemon_wal_apply_ms.is_some());
        assert_eq!(
            cold.daemon_decrypt_ms,
            Some(cold.daemon_db_decrypt_ms.unwrap() + cold.daemon_wal_apply_ms.unwrap())
        );
        assert_eq!(cold.latest_timestamp, Some(400));
        let previous_wal_time = std::fs::metadata(&wal_path).unwrap().modified().unwrap();
        let second = latency_wal_bytes(&latency_encrypted_pages(
            &temp.path().join("wal-second.db"),
            &[700],
            &[0x42; 32],
            true,
        ));
        std::fs::write(&wal_path, &second).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&wal_path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(previous_wal_time + Duration::from_secs(1)),
            )
            .unwrap();
        let incremental = cache.latency_probe(10).await.unwrap();
        incremental.validate(10).unwrap();
        assert_eq!(incremental.cache_mode, "wal_incremental");
        assert!(incremental.daemon_db_decrypt_ms.is_none());
        assert!(incremental.daemon_wal_apply_ms.is_some());
        assert_eq!(
            incremental.daemon_decrypt_ms,
            incremental.daemon_wal_apply_ms
        );
        assert_eq!(incremental.latest_timestamp, Some(700));
        assert_eq!(std::fs::read(&source).unwrap(), encrypted);
        assert_eq!(std::fs::read(&wal_path).unwrap(), second);

        std::fs::remove_file(&wal_path).unwrap();
        let absent = cache.latency_probe(10).await.unwrap();
        absent.validate(10).unwrap();
        assert_eq!(absent.cache_mode, "wal_incremental");
        assert!(absent.daemon_decrypt_ms.is_none());
        assert_eq!(
            absent.decrypt_skipped_reason.as_deref(),
            Some("wal_absent_no_decryption")
        );
        assert_eq!(absent.latest_timestamp, Some(700));
    }

    #[tokio::test]
    async fn latency_probe_bounds_rows_and_reports_measured_empty_query() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, _, _) = latency_fixture(&temp, &[0, -1]).await;
        assert!(cache.latency_probe(0).await.is_err());
        assert!(cache.latency_probe(10_001).await.is_err());
        assert!(!cache.cache_file_path("session/session.db").exists());
        let empty = cache.latency_probe(5).await.unwrap();
        empty.validate(5).unwrap();
        assert_eq!(empty.rows_read, 0);
        assert!(empty.latest_timestamp.is_none());
        assert!(!empty.possible_truncation);
        assert!(empty.daemon_query_ms.is_finite() && empty.daemon_query_ms >= 0.0);
    }

    #[tokio::test]
    async fn latency_probe_propagates_query_failure_without_fabricated_report() {
        let temp = tempfile::tempdir().unwrap();
        let (cache, _, _) = latency_fixture(&temp, &[100]).await;
        cache.latency_probe(10).await.unwrap();
        let output = cache.cache_file_path("session/session.db");
        let conn = rusqlite::Connection::open(&output).unwrap();
        conn.execute_batch("DROP TABLE SessionTable;").unwrap();
        drop(conn);
        assert!(cache.latency_probe(10).await.is_err());
    }

    #[tokio::test]
    async fn image_output_paths_include_context_cached_files_and_refuse_busy_metadata() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let cached = root.path().join("cache");
        let mtime = cached.join("_mtimes.json");
        std::fs::create_dir(&source).unwrap();
        let mut cache = DbCache::with_dirs(
            source.clone(),
            cached.clone(),
            mtime.clone(),
            HashMap::new(),
        )
        .await
        .unwrap();
        let keys = root.path().join("keys.json");
        let config = root.path().join("config.json");
        let decrypted = root.path().join("decrypted");
        std::fs::write(&keys, b"secret sentinel").unwrap();
        cache.output_protected_paths = vec![keys.clone(), config.clone(), decrypted.clone()];
        let outside = root.path().join("outside-cache.db");
        {
            let mut inner = cache.inner.lock().await;
            inner.insert(
                "message/message_0.db".into(),
                CacheEntry {
                    db_mtime: 1,
                    wal_mtime: 0,
                    decrypted_path: outside.clone(),
                },
            );
            assert!(cache.output_protection_paths().is_err());
        }
        let paths = cache.output_protection_paths().unwrap();
        for path in [
            source,
            cached,
            mtime,
            keys.clone(),
            config,
            decrypted,
            outside,
        ] {
            assert!(paths.contains(&path));
        }
        assert_eq!(paths.len(), 7);
        assert_eq!(std::fs::read(keys).unwrap(), b"secret sentinel");
    }

    #[test]
    fn media_db_keys_preserves_raw_keys_and_filters_exact_media_names() {
        let accepted = [
            "message/media_0.db",
            "message\\MEDIA_0.DB",
            "MESSAGE/media_12.db",
            "message/media_001.db",
        ];
        let rejected = [
            "message/message_0.db",
            "contact/media_0.db",
            "message/media_cache.db",
            "message/media_.db",
            "message/media_1.db-wal",
            "message/media_1.db-shm",
            "message/media_１.db",
            "message/media_../1.db",
            "media_0.db",
        ];
        let all_keys = accepted
            .iter()
            .chain(rejected.iter())
            .map(|k| (k.to_string(), "synthetic-value-not-returned".into()))
            .collect();
        // 直接构造只读状态；不加载运行时、不创建目录、不解密。
        let cache = DbCache {
            db_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            mtime_file: PathBuf::new(),
            output_protected_paths: Vec::new(),
            all_keys,
            inner: Arc::new(Mutex::new(HashMap::new())),
            work: Arc::new(Mutex::new(())),
            before_commit: Default::default(),
        };
        let mut expected: Vec<String> = accepted.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(cache.media_db_keys(), expected);
        assert_eq!(cache.media_db_keys(), expected);
    }

    /// 仅用于合成数据库，不包含账号密钥。
    const FAKE_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// 缓存使用不同于源库的时间戳，误走全量解密时字节比较必定失败。
    fn original_cached_bytes() -> &'static [u8] {
        static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("cached.db");
            let _encrypted = latency_encrypted_pages(&path, &[999], &[0; 32], false);
            std::fs::read(path).unwrap()
        });
        BYTES.as_slice()
    }

    fn write_encrypted_fixture(path: &Path, timestamp: i64) {
        let plain = tempfile::tempdir().unwrap();
        let encrypted = latency_encrypted_pages(
            &plain.path().join("plain.db"),
            &[timestamp],
            &[0; 32],
            false,
        );
        std::fs::write(path, encrypted).unwrap();
    }

    fn unique_tmpdir(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("wx-cli-cache-test-{}-{}-{}", tag, pid, nanos));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 准备一份 "DbCache 已经 reuse 了 cached 解密产物" 的初始状态。
    /// 返回 (cache, db_path, decrypted_path, mtime_file, rel_key)。
    async fn setup_seeded_cache(tag: &str) -> (DbCache, PathBuf, PathBuf, PathBuf, String) {
        let root = unique_tmpdir(tag);
        let db_dir = root.join("db_storage");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let rel_key = "message_0.db".to_string();
        let db_path = db_dir.join(&rel_key);
        write_encrypted_fixture(&db_path, 100);

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, original_cached_bytes()).unwrap();

        let db_mt = mtime_nanos(&db_path);
        let mtime_file = cache_dir.join("_mtimes.json");
        let payload = serde_json::to_string(&serde_json::json!({
            &rel_key: {
                "db_mt": db_mt,
                "wal_mt": 0u64,
                "path": decrypted_path.display().to_string(),
            }
        }))
        .unwrap();
        std::fs::write(&mtime_file, payload).unwrap();

        let mut all_keys = HashMap::new();
        all_keys.insert(rel_key.clone(), FAKE_KEY_HEX.to_string());
        let cache = DbCache::with_dirs(db_dir, cache_dir, mtime_file.clone(), all_keys)
            .await
            .unwrap();

        (cache, db_path, decrypted_path, mtime_file, rel_key)
    }

    #[tokio::test]
    async fn exact_mtime_hit_skips_decrypt() {
        let (cache, _db_path, decrypted_path, _mtime_file, rel_key) =
            setup_seeded_cache("exact").await;

        let p = cache
            .get(&rel_key)
            .await
            .unwrap()
            .expect("cache should hit");
        assert_eq!(p, decrypted_path);

        // 完全 hit → cached file 内容不应被改
        let body = std::fs::read(&decrypted_path).unwrap();
        assert_eq!(body, original_cached_bytes());
    }

    #[tokio::test]
    async fn invalidate_removes_memory_and_persistent_entries() {
        let (cache, _db_path, _decrypted_path, mtime_file, rel_key) =
            setup_seeded_cache("invalidate").await;

        assert!(cache.invalidate(&rel_key).await);
        assert!(!cache.inner.lock().await.contains_key(&rel_key));

        let persisted: HashMap<String, MtimeEntry> =
            serde_json::from_str(&std::fs::read_to_string(mtime_file).unwrap()).unwrap();
        assert!(!persisted.contains_key(&rel_key));
        assert!(!cache.invalidate(&rel_key).await);
    }

    #[tokio::test]
    async fn wal_only_change_uses_incremental_path() {
        // 自己构造（不走 setup_seeded_cache）以便初始 mtime.json 同时写 db_mt 和 wal_mt
        let root = unique_tmpdir("walonly");
        let db_dir = root.join("db_storage");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let rel_key = "message_0.db".to_string();
        let db_path = db_dir.join(&rel_key);
        write_encrypted_fixture(&db_path, 100);

        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, original_cached_bytes()).unwrap();

        let db_mt = mtime_nanos(&db_path);
        let wal_mt0 = mtime_nanos(&wal_path);
        let mtime_file = cache_dir.join("_mtimes.json");
        let payload = serde_json::to_string(&serde_json::json!({
            &rel_key: {
                "db_mt": db_mt,
                "wal_mt": wal_mt0,
                "path": decrypted_path.display().to_string(),
            }
        }))
        .unwrap();
        std::fs::write(&mtime_file, payload).unwrap();

        let mut all_keys = HashMap::new();
        all_keys.insert(rel_key.clone(), FAKE_KEY_HEX.to_string());
        let cache = DbCache::with_dirs(db_dir, cache_dir, mtime_file, all_keys)
            .await
            .unwrap();

        // 第一次：完全 hit
        let p1 = cache.get(&rel_key).await.unwrap().expect("first get hits");
        assert_eq!(p1, decrypted_path);
        assert_eq!(
            std::fs::read(&decrypted_path).unwrap(),
            original_cached_bytes()
        );

        // 合法空 WAL 只改变修改时间，不包含可提交页面。
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();
        let wal_mt1 = mtime_nanos(&wal_path);
        assert_ne!(wal_mt0, wal_mt1, "rewriting WAL should bump mtime");

        // 第二次：WAL 增量路径
        // 误走全量解密会把缓存中的 999 替换为源库中的 100。
        let p2 = cache
            .get(&rel_key)
            .await
            .unwrap()
            .expect("WAL-incremental path should produce path");
        assert_eq!(p2, decrypted_path);

        let body = std::fs::read(&decrypted_path).unwrap();
        assert_eq!(
            body,
            original_cached_bytes(),
            "WAL-incremental should NOT rewrite cached file"
        );
    }

    #[tokio::test]
    async fn db_mtime_change_triggers_full_decrypt() {
        let (cache, db_path, decrypted_path, _mtime_file, rel_key) =
            setup_seeded_cache("dbchange").await;

        // bump 主 .db 的 mtime（重写一份不同 bytes）
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_encrypted_fixture(&db_path, 200);
        assert_ne!(
            mtime_nanos(&db_path),
            cache.inner.lock().await.get(&rel_key).unwrap().db_mtime,
            "rewriting db file should bump mtime"
        );

        // 源库变化后应完整解密，将缓存时间戳更新为 200。
        let p = cache
            .get(&rel_key)
            .await
            .unwrap()
            .expect("cache should produce path");
        assert_eq!(p, decrypted_path);

        let new_size = std::fs::metadata(&decrypted_path).unwrap().len() as usize;
        assert!(
            new_size >= crate::crypto::PAGE_SZ,
            "expected full_decrypt to rewrite cached file to PAGE_SZ multiple, got size={}",
            new_size,
        );
        let conn = rusqlite::Connection::open_with_flags(
            &decrypted_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let timestamp: i64 = conn
            .query_row("SELECT last_timestamp FROM SessionTable", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(timestamp, 200);
    }

    #[tokio::test]
    async fn get_with_mode_reports_each_path() {
        let root = unique_tmpdir("getwithmode");
        let db_dir = root.join("db_storage");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let rel_key = "message_0.db".to_string();
        let db_path = db_dir.join(&rel_key);
        write_encrypted_fixture(&db_path, 100);
        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, original_cached_bytes()).unwrap();

        let db_mt = mtime_nanos(&db_path);
        let wal_mt0 = mtime_nanos(&wal_path);
        let mtime_file = cache_dir.join("_mtimes.json");
        let payload = serde_json::to_string(&serde_json::json!({
            &rel_key: {
                "db_mt": db_mt,
                "wal_mt": wal_mt0,
                "path": decrypted_path.display().to_string(),
            }
        }))
        .unwrap();
        std::fs::write(&mtime_file, payload).unwrap();

        let mut all_keys = HashMap::new();
        all_keys.insert(rel_key.clone(), FAKE_KEY_HEX.to_string());
        let cache = DbCache::with_dirs(db_dir, cache_dir, mtime_file, all_keys)
            .await
            .unwrap();

        let hit = cache
            .get_with_mode(&rel_key)
            .await
            .unwrap()
            .expect("cache should hit");
        assert_eq!(hit.path, decrypted_path);
        assert_eq!(hit.mode, CacheMode::CacheHit);

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();
        let wal = cache
            .get_with_mode(&rel_key)
            .await
            .unwrap()
            .expect("WAL-only change should stay incremental");
        assert_eq!(wal.path, decrypted_path);
        assert_eq!(wal.mode, CacheMode::WalIncremental);

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_encrypted_fixture(&db_path, 200);
        let full = cache
            .get_with_mode(&rel_key)
            .await
            .unwrap()
            .expect("db mtime change should trigger full decrypt");
        assert_eq!(full.path, decrypted_path);
        assert_eq!(full.mode, CacheMode::FullDecrypt);
    }

    #[tokio::test]
    async fn restart_with_wal_change_still_reuses_cached_db_then_applies_wal() {
        let root = unique_tmpdir("restart-wal");
        let db_dir = root.join("db_storage");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let rel_key = "message_0.db".to_string();
        let db_path = db_dir.join(&rel_key);
        write_encrypted_fixture(&db_path, 100);

        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, original_cached_bytes()).unwrap();

        let db_mt = mtime_nanos(&db_path);
        let wal_mt0 = mtime_nanos(&wal_path);
        let mtime_file = cache_dir.join("_mtimes.json");
        let payload = serde_json::to_string(&serde_json::json!({
            &rel_key: {
                "db_mt": db_mt,
                "wal_mt": wal_mt0,
                "path": decrypted_path.display().to_string(),
            }
        }))
        .unwrap();
        std::fs::write(&mtime_file, payload).unwrap();

        // 模拟 daemon 重启前又有新消息写入 WAL
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&wal_path, latency_wal_bytes(&[])).unwrap();
        let wal_mt1 = mtime_nanos(&wal_path);
        assert_ne!(wal_mt0, wal_mt1);

        let mut all_keys = HashMap::new();
        all_keys.insert(rel_key.clone(), FAKE_KEY_HEX.to_string());
        let cache = DbCache::with_dirs(db_dir, cache_dir, mtime_file, all_keys)
            .await
            .unwrap();

        let p = cache
            .get(&rel_key)
            .await
            .unwrap()
            .expect("cache should reuse persisted DB");
        assert_eq!(p, decrypted_path);
        let body = std::fs::read(&decrypted_path).unwrap();
        assert_eq!(
            body,
            original_cached_bytes(),
            "restart + WAL-only change should still reuse cached DB and avoid full_decrypt"
        );
    }
}
