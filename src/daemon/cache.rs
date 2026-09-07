use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::crypto;
use crate::crypto::wal;
use crate::runtime::RuntimeContext;

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
}

impl DbCache {
    pub async fn new(db_dir: PathBuf, all_keys: HashMap<String, String>) -> Result<Self> {
        let runtime = RuntimeContext::load()?;
        let mut cache =
            Self::with_dirs(db_dir, runtime.cache_dir(), runtime.mtime_file(), all_keys).await?;
        // 沿用初始化时的账号上下文，图片查询不得再加载可能已切换的配置。
        cache.output_protected_paths = vec![
            runtime.config_path,
            runtime.config.keys_file,
            runtime.config.decrypted_dir,
        ];
        Ok(cache)
    }

    /// 注入 `cache_dir` / `mtime_file`（测试用 + 生产 `new()` 复用）
    pub(crate) async fn with_dirs(
        db_dir: PathBuf,
        cache_dir: PathBuf,
        mtime_file: PathBuf,
        all_keys: HashMap<String, String>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(&cache_dir).await?;

        let cache = DbCache {
            db_dir,
            cache_dir,
            mtime_file,
            output_protected_paths: Vec::new(),
            all_keys,
            inner: Arc::new(Mutex::new(HashMap::new())),
        };

        cache.load_persistent().await;
        Ok(cache)
    }

    /// 数据库根目录（即 `<wxchat_base>/db_storage`）。
    /// 上层（attachment resolver）需要 `db_dir.parent()` 来定位 `msg/attach/...` 解密图片。
    pub fn db_dir(&self) -> &Path {
        &self.db_dir
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
        let removed = {
            let mut inner = self.inner.lock().await;
            inner.remove(rel_key).is_some()
        };
        if removed {
            self.save_persistent().await;
        }
        removed
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
            let db_path = self.db_dir.join(
                rel_key
                    .replace('\\', std::path::MAIN_SEPARATOR_STR)
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            );
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

    /// 持久化 mtime 记录
    async fn save_persistent(&self) {
        let mtime_file = &self.mtime_file;
        let inner = self.inner.lock().await;
        let data: HashMap<String, MtimeEntry> = inner
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    MtimeEntry {
                        db_mt: v.db_mtime,
                        wal_mt: v.wal_mtime,
                        path: v.decrypted_path.to_string_lossy().into_owned(),
                    },
                )
            })
            .collect();
        drop(inner);

        if let Ok(json) = serde_json::to_string_pretty(&data) {
            let _ = tokio::fs::write(&mtime_file, json).await;
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
    /// WeChat 在写消息时只 append WAL（除非触发 checkpoint），因此 path 2 是常态；
    /// 这条路径把"每次请求都全量解密 ~1.8GB DB（~120s）"压到"只解 WAL 帧（典型 < 10s）"。
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

        let db_path = self.db_dir.join(
            rel_key
                .replace('\\', std::path::MAIN_SEPARATOR_STR)
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        if !db_path.exists() {
            return Ok(None);
        }

        let wal_path = wal_path_for(&db_path);
        let db_mt = mtime_nanos(&db_path);
        let wal_mt = if wal_path.exists() {
            mtime_nanos(&wal_path)
        } else {
            0
        };

        let cached = {
            let inner = self.inner.lock().await;
            inner.get(rel_key).cloned()
        };

        let enc_key_bytes =
            hex_to_32bytes(&enc_key_hex).with_context(|| format!("密钥格式错误: {}", rel_key))?;

        // Path 1 / Path 2：主 .db mtime 未变且 cached 产物仍在
        if let Some(entry) = cached.as_ref() {
            if entry.db_mtime == db_mt && entry.decrypted_path.exists() {
                if entry.wal_mtime == wal_mt {
                    return Ok(Some(TimedCacheResolve {
                        resolved: CacheResolve {
                            path: entry.decrypted_path.clone(),
                            mode: CacheMode::CacheHit,
                        },
                        timing: CacheTimings {
                            resolve: resolve_started.elapsed(),
                            db_decrypt: None,
                            wal_apply: None,
                        },
                    }));
                }

                // Path 2: WAL-only 变化 → 在 cached 产物上重新 apply_wal
                // 不存在的 WAL 也要更新 wal_mtime=0（虽然 SQLite 不会自发"主库不变 + WAL 清空"）
                let out_path = entry.decrypted_path.clone();
                let t0 = std::time::Instant::now();
                let mut wal_apply = None;
                if wal_path.exists() {
                    let out_path2 = out_path.clone();
                    let wal_path2 = wal_path.clone();
                    let key_copy = enc_key_bytes;
                    wal_apply = Some(
                        tokio::task::spawn_blocking(move || {
                            let phase = Instant::now();
                            wal::apply_wal(&wal_path2, &out_path2, &key_copy)?;
                            Ok::<_, anyhow::Error>(phase.elapsed())
                        })
                        .await??,
                    );
                }
                eprintln!(
                    "[cache] WAL 增量 {} ({}ms)",
                    rel_key,
                    t0.elapsed().as_millis()
                );

                {
                    let mut inner = self.inner.lock().await;
                    inner.insert(
                        rel_key.to_string(),
                        CacheEntry {
                            db_mtime: db_mt,
                            wal_mtime: wal_mt,
                            decrypted_path: out_path.clone(),
                        },
                    );
                }
                self.save_persistent().await;
                return Ok(Some(TimedCacheResolve {
                    resolved: CacheResolve {
                        path: out_path,
                        mode: CacheMode::WalIncremental,
                    },
                    timing: CacheTimings {
                        resolve: resolve_started.elapsed(),
                        db_decrypt: None,
                        wal_apply,
                    },
                }));
            }
        }

        // Path 3: 主 .db 变了 / 缓存 miss → 全量解密
        let out_path = self.cache_file_path(rel_key);
        let t0 = std::time::Instant::now();
        let db_path2 = db_path.clone();
        let out_path2 = out_path.clone();
        let key_copy = enc_key_bytes;
        let db_decrypt = tokio::task::spawn_blocking(move || {
            let phase = Instant::now();
            crypto::full_decrypt(&db_path2, &out_path2, &key_copy)?;
            Ok::<_, anyhow::Error>(phase.elapsed())
        })
        .await??;

        let mut wal_apply = None;
        if wal_path.exists() {
            let out_path3 = out_path.clone();
            let wal_path3 = wal_path.clone();
            let key_copy2 = enc_key_bytes;
            wal_apply = Some(
                tokio::task::spawn_blocking(move || {
                    let phase = Instant::now();
                    wal::apply_wal(&wal_path3, &out_path3, &key_copy2)?;
                    Ok::<_, anyhow::Error>(phase.elapsed())
                })
                .await??,
            );
        }

        eprintln!(
            "[cache] 全量解密 {} ({}ms)",
            rel_key,
            t0.elapsed().as_millis()
        );

        {
            let mut inner = self.inner.lock().await;
            inner.insert(
                rel_key.to_string(),
                CacheEntry {
                    db_mtime: db_mt,
                    wal_mtime: wal_mt,
                    decrypted_path: out_path.clone(),
                },
            );
        }

        self.save_persistent().await;
        Ok(Some(TimedCacheResolve {
            resolved: CacheResolve {
                path: out_path,
                mode: CacheMode::FullDecrypt,
            },
            timing: CacheTimings {
                resolve: resolve_started.elapsed(),
                db_decrypt: Some(db_decrypt),
                wal_apply,
            },
        }))
    }

    /// 延迟专用探测：复用真实缓存加载，再查询固定数据库；不预热、不强制失效、不接受任意 SQL。
    pub async fn latency_probe(&self, limit: usize) -> Result<LatencyProbe> {
        ensure!(
            (1..=10_000).contains(&limit),
            "延迟探测行数上限须在 1..10000 内"
        );
        let resolved = self
            .get_with_timing("session/session.db")
            .await?
            .context("延迟探测无法加载当前账号 session.db")?;
        let path = resolved.resolved.path;
        let (rows_read, latest_timestamp, query_duration) = tokio::task::spawn_blocking(move || {
            let started = Instant::now();
            let (count, latest) = {
                let conn = rusqlite::Connection::open_with_flags(&path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
                conn.busy_timeout(Duration::from_secs(1))?;
                let mut statement = conn.prepare(
                    "SELECT last_timestamp FROM SessionTable WHERE last_timestamp > 0 ORDER BY last_timestamp DESC LIMIT ?1"
                )?;
                let mut rows = statement.query([limit as i64])?;
                let mut count = 0;
                let mut latest: Option<i64> = None;
                while let Some(row) = rows.next()? {
                    let timestamp: i64 = row.get(0)?;
                    ensure!(timestamp > 0, "延迟探测查询返回无效时间戳");
                    count += 1;
                    latest = Some(latest.map_or(timestamp, |old| old.max(timestamp)));
                }
                (count, latest)
            };
            Ok::<_, anyhow::Error>((count, latest, started.elapsed()))
        }).await??;
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

    fn latency_wal_bytes(pages: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 32];
        bytes[..4].copy_from_slice(&0x377f0682u32.to_be_bytes());
        bytes[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&(crypto::PAGE_SZ as u32).to_be_bytes());
        bytes[16..24].copy_from_slice(&[7; 8]);
        for (index, page) in pages.chunks_exact(crypto::PAGE_SZ).enumerate() {
            let mut frame = [0u8; 24];
            frame[..4].copy_from_slice(&((index + 1) as u32).to_be_bytes());
            frame[4..8].copy_from_slice(&((pages.len() / crypto::PAGE_SZ) as u32).to_be_bytes());
            frame[8..16].copy_from_slice(&[7; 8]);
            bytes.extend(frame);
            bytes.extend(page);
        }
        bytes
    }

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

        // 保留源 mtime 后破坏合成密文；后续还能查询，证明命中路径没有重新解密。
        let source_modified = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, vec![0u8; encrypted.len()]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(source_modified))
            .unwrap();
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
        };
        let mut expected: Vec<String> = accepted.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(cache.media_db_keys(), expected);
        assert_eq!(cache.media_db_keys(), expected);
    }

    /// 64 字符 hex（不需要是真 SQLCipher key — 仅用来证明"是否触发了 full_decrypt"）
    const FAKE_KEY_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// 路径区分约定：
    /// - 完全 hit / WAL 增量 → `decrypted_path` **内容不变**
    /// - 全量解密 → `crypto::full_decrypt` 把 cached file **重写为 PAGE_SZ 倍数**
    ///   （fake key 解出 4096 字节垃圾，但仍写入 — 不验证内容合法性）
    /// 因此用 cached file 的"size 是否被改"来判断走了哪条路径。
    const ORIGINAL_CACHED_BYTES: &[u8] = b"original cached contents";

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
        std::fs::write(&db_path, b"fake encrypted db").unwrap();

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, ORIGINAL_CACHED_BYTES).unwrap();

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
        assert_eq!(body, ORIGINAL_CACHED_BYTES);
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
        std::fs::write(&db_path, b"fake encrypted db").unwrap();

        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, [0u8; 31]).unwrap(); // ≤ WAL_HDR_SZ=32 → apply_wal noop

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, ORIGINAL_CACHED_BYTES).unwrap();

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
            ORIGINAL_CACHED_BYTES
        );

        // bump WAL mtime（重写仍 31 bytes，apply_wal 仍 noop）
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&wal_path, [0xffu8; 31]).unwrap();
        let wal_mt1 = mtime_nanos(&wal_path);
        assert_ne!(wal_mt0, wal_mt1, "rewriting WAL should bump mtime");

        // 第二次：WAL 增量路径
        // 如果错误地走 full_decrypt → cached file 大小会被重写为 ≥ PAGE_SZ
        let p2 = cache
            .get(&rel_key)
            .await
            .unwrap()
            .expect("WAL-incremental path should produce path");
        assert_eq!(p2, decrypted_path);

        let body = std::fs::read(&decrypted_path).unwrap();
        assert_eq!(
            body, ORIGINAL_CACHED_BYTES,
            "WAL-incremental should NOT rewrite cached file"
        );
    }

    #[tokio::test]
    async fn db_mtime_change_triggers_full_decrypt() {
        let (cache, db_path, decrypted_path, _mtime_file, rel_key) =
            setup_seeded_cache("dbchange").await;

        // bump 主 .db 的 mtime（重写一份不同 bytes）
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&db_path, b"different fake encrypted bytes").unwrap();
        assert_ne!(
            mtime_nanos(&db_path),
            cache.inner.lock().await.get(&rel_key).unwrap().db_mtime,
            "rewriting db file should bump mtime"
        );

        // 走 full_decrypt 路径 → fake key 不会让 full_decrypt 失败（它不验证内容），
        // 但会把 cached file 重写为 PAGE_SZ 倍数。原始内容是 24 bytes，重写后应该 ≥ 4096 bytes。
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
        std::fs::write(&db_path, b"fake encrypted db").unwrap();
        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, [0u8; 31]).unwrap();

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, ORIGINAL_CACHED_BYTES).unwrap();

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
        std::fs::write(&wal_path, [0xffu8; 31]).unwrap();
        let wal = cache
            .get_with_mode(&rel_key)
            .await
            .unwrap()
            .expect("WAL-only change should stay incremental");
        assert_eq!(wal.path, decrypted_path);
        assert_eq!(wal.mode, CacheMode::WalIncremental);

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&db_path, b"different bytes").unwrap();
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
        std::fs::write(&db_path, b"fake encrypted db").unwrap();

        let wal_path = wal_path_for(&db_path);
        std::fs::write(&wal_path, [0u8; 31]).unwrap(); // WAL 增量仍是 noop

        let cached_hash = format!("{:x}", md5::compute(rel_key.as_bytes()));
        let decrypted_path = cache_dir.join(format!("{}.db", cached_hash));
        std::fs::write(&decrypted_path, ORIGINAL_CACHED_BYTES).unwrap();

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
        std::fs::write(&wal_path, [0xffu8; 31]).unwrap();
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
            body, ORIGINAL_CACHED_BYTES,
            "restart + WAL-only change should still reuse cached DB and avoid full_decrypt"
        );
    }
}
