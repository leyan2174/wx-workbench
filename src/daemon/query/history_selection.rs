//! 历史查询的 SQL 选行与跨分片分页；账号、连接、发送者及正文渲染仍由调用方负责。
use anyhow::{ensure, Context, Result};
use rusqlite::{params_from_iter, Connection, Row};

/// 同时间记录保持调用方的分片顺序及 SQLite 返回顺序，与旧稳定排序一致。
/// SQLite 未定义同时间行的唯一顺序；本助手不虚构跨快照稳定游标。
pub struct Ranked<T> {
    pub timestamp: i64,
    pub value: T,
}

#[derive(Clone)]
pub struct Selection {
    limit: usize,
    offset: usize,
    candidate_limit: i64,
    since: Option<i64>,
    until: Option<i64>,
    types: Vec<i64>,
    oldest_first: bool,
}

impl Selection {
    /// 空类型列表表示不限类型。base 类型匹配低 32 位；完整高位类型精确匹配。
    /// 时间为闭区间；日期字符串及 msg_types 名称映射由现有协议层处理。
    pub fn new(
        limit: usize,
        offset: usize,
        since: Option<i64>,
        until: Option<i64>,
        types: &[i64],
        oldest_first: bool,
    ) -> Result<Self> {
        ensure!(limit > 0, "history limit must be positive");
        ensure!(
            since.zip(until).is_none_or(|(s, u)| s <= u),
            "invalid history time range"
        );
        let candidate_limit =
            i64::try_from(offset.checked_add(limit).context("history page overflow")?)
                .context("history page exceeds SQLite integer range")?;
        let mut types = types.to_vec();
        types.sort_unstable();
        types.dedup();
        // IPC 最多 100 个名称；避免独立调用方构造超过 SQLite 绑定上限的列表。
        ensure!(types.len() <= 100, "too many history types");
        Ok(Self {
            limit,
            offset,
            candidate_limit,
            since,
            until,
            types,
            oldest_first,
        })
    }

    /// 在已选定账号的连接和已发现消息表上取候选；不在单个分片应用全局 offset。
    /// map 看到与原 query_messages 相同的六列，允许复用 get_content_bytes 等原映射。
    /// SQL/映射失败向上传递，不能把坏分片静默当作空分片。
    pub fn query_shard<T>(
        &self,
        conn: &Connection,
        table: &str,
        mut map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<Ranked<T>>> {
        ensure!(
            table.strip_prefix("Msg_").is_some_and(|hash| {
                hash.len() == 32
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }),
            "invalid message table name"
        );
        let mut clauses = Vec::new();
        let mut params = Vec::new();
        if let Some(since) = self.since {
            clauses.push("create_time >= ?".to_owned());
            params.push(since);
        }
        if let Some(until) = self.until {
            clauses.push("create_time <= ?".to_owned());
            params.push(until);
        }
        if !self.types.is_empty() {
            let mut predicates = Vec::new();
            for &kind in &self.types {
                predicates.push(if (0..=u32::MAX as i64).contains(&kind) {
                    "(local_type & 4294967295) = ?"
                } else {
                    "local_type = ?"
                });
                params.push(kind);
            }
            clauses.push(format!("({})", predicates.join(" OR ")));
        }
        let filter = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let order = if self.oldest_first { "ASC" } else { "DESC" };
        let sql = format!("SELECT local_id, local_type, create_time, real_sender_id, message_content, WCDB_CT_message_content FROM [{table}] {filter} ORDER BY create_time {order} LIMIT ?");
        params.push(self.candidate_limit);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(params), |row| {
                Ok(Ranked {
                    timestamp: row.get(2)?,
                    value: map(row)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 按调用方的分片顺序 extend 各分片候选后调用；两种选页方向均按时间升序展示。
    /// 不按 local_id 去重：不同分片可能合法复用同一个 local_id。
    pub fn page<T>(&self, mut candidates: Vec<Ranked<T>>) -> Vec<T> {
        candidates.sort_by(|a, b| {
            if self.oldest_first {
                a.timestamp.cmp(&b.timestamp)
            } else {
                b.timestamp.cmp(&a.timestamp)
            }
        });
        let mut page: Vec<_> = candidates
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .collect();
        page.sort_by_key(|entry| entry.timestamp);
        page.into_iter().map(|entry| entry.value).collect()
    }
}
