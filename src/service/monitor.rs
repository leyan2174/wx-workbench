//! 大游标只分块传输，不分块查询。限额按 JSON 的 UTF-8 字节数计算。
use serde::{Deserialize, Serialize};

pub const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SESSIONS: usize = 100_000;
pub const CHUNK_BYTES: usize = 48 * 1024;
pub const MAX_CHUNK_ENTRIES: usize = 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

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
