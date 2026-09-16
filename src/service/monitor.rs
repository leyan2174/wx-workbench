//! 大游标只分块传输，不分块查询。限额按 JSON 的 UTF-8 字节数计算。
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SESSIONS: usize = 100_000;
pub const CHUNK_BYTES: usize = 48 * 1024;
pub const MAX_CHUNK_ENTRIES: usize = 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
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
        let expected_decrypt = measured_total(self.daemon_db_decrypt_ms, self.daemon_wal_apply_ms);
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

pub fn measured_total(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first + second),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Call {
    Begin {
        sessions: usize,
        bytes: usize,
    },
    Chunk {
        id: String,
        sequence: u32,
        entries: Vec<(String, i64)>,
    },
    Finish {
        id: String,
        chunks: u32,
        limit: usize,
        with_meta: bool,
        debug_source: bool,
        max_response_bytes: usize,
    },
    Abort {
        id: String,
    },
}

impl Call {
    pub fn response_limit(&self) -> usize {
        match self {
            Self::Finish {
                max_response_bytes, ..
            } => (*max_response_bytes)
                .clamp(1024, MAX_RESPONSE_BYTES)
                .saturating_add(4096),
            _ => 4096,
        }
    }
}

/// 对象成员长度不含逗号和外层花括号；serde 负责转义，不能用字符数估算。
pub fn entry_bytes(name: &str, timestamp: i64) -> usize {
    serde_json::to_vec(name)
        .expect("string serialization")
        .len()
        + 1
        + timestamp.to_string().len()
}

pub fn valid_entry(name: &str, timestamp: i64, latest: i64) -> bool {
    !name.trim().is_empty()
        && name.len() <= 512
        && !name.chars().any(char::is_control)
        && (0..=latest).contains(&timestamp)
}
