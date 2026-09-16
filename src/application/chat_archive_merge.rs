//! 聊天 JSON 增量合并；不访问文件、数据库、时钟或用户数据。
//!
//! 调用方负责生成稳定的非空字符串 source（必须包含分片身份），更新导出日期。
//! 旧消息永不覆盖，旧顶层字段优先，incoming 仅补充缺失字段。
//! 与 Python export_one 的差异：按 source + local_id 去重，新增批次也去重，
//! 全部消息稳定排序；未知 source 的同号碰撞原子失败，绝不猜测身份。

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MergeReport {
    /// 成功时追加的消息数；失败时为 0。
    pub added: usize,
    /// 原有消息数，包含原有重复；从不清理用户的历史记录。
    pub retained: usize,
    /// 成功时跳过的 incoming 消息数；失败时为 0。
    pub duplicates: usize,
    /// 无法安全确定身份的 local_id 分组数。
    pub ambiguous: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageLocation {
    /// "existing" 或 "incoming"。
    pub input: &'static str,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ambiguity {
    pub local_id: Value,
    pub locations: Vec<MessageLocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum MergeError {
    InvalidInput {
        path: String,
        reason: String,
    },
    UsernameMismatch {
        existing: String,
        incoming: String,
    },
    /// 无部分结果；报告可供调用方阻止写入并请求补充分片信息。
    Ambiguous {
        report: MergeReport,
        conflicts: Vec<Ambiguity>,
    },
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { path, reason } => write!(f, "{path}: {reason}"),
            Self::UsernameMismatch { .. } => write!(f, "chat username mismatch"),
            Self::Ambiguous { report, .. } => {
                write!(
                    f,
                    "{} ambiguous local_id groups: source required",
                    report.ambiguous
                )
            }
        }
    }
}

impl std::error::Error for MergeError {}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeResult {
    pub document: Value,
    pub report: MergeReport,
}

fn invalid(path: impl Into<String>, reason: &str) -> MergeError {
    MergeError::InvalidInput {
        path: path.into(),
        reason: reason.into(),
    }
}

struct Message<'a> {
    value: &'a Value,
    id: String,
    source: Option<&'a str>,
    timestamp: i128,
    location: MessageLocation,
}

fn read_messages<'a>(
    doc: &'a Value,
    input: &'static str,
) -> Result<(&'a str, Vec<Message<'a>>), MergeError> {
    let object = doc
        .as_object()
        .ok_or_else(|| invalid(input, "expected object"))?;
    let username = object
        .get("username")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| invalid(format!("{input}.username"), "expected nonempty string"))?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("{input}.messages"), "expected array"))?;
    let parsed = messages
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let path = format!("{input}.messages[{index}]");
            let obj = value
                .as_object()
                .ok_or_else(|| invalid(&path, "expected object"))?;
            let id = obj
                .get("local_id")
                .ok_or_else(|| invalid(format!("{path}.local_id"), "required"))?;
            if !(id.as_i64().is_some()
                || id.as_u64().is_some()
                || id.as_str().is_some_and(|s| !s.trim().is_empty()))
            {
                return Err(invalid(
                    format!("{path}.local_id"),
                    "expected integer or nonempty string",
                ));
            }
            let source = match obj.get("source") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                _ => {
                    return Err(invalid(
                        format!("{path}.source"),
                        "expected nonempty string or null",
                    ))
                }
            };
            let timestamp = match obj.get("timestamp") {
                None | Some(Value::Null) => 0,
                Some(v) => v
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
                    .ok_or_else(|| {
                        invalid(format!("{path}.timestamp"), "expected integer or null")
                    })?,
            };
            Ok(Message {
                value,
                id: id.to_string(),
                source,
                timestamp,
                location: MessageLocation { input, index },
            })
        })
        .collect::<Result<Vec<_>, MergeError>>()?;
    Ok((username, parsed))
}

/// 两个文档必须具有相同的非空 username 和 messages 数组。
/// local_id 支持整数和非空字符串，两种 JSON 类型不互相等同。
/// 时间戳支持 i64/u64 整数；缺失/NULL 为排序键 0，不改写原字段。
/// 任何碰撞组包含未知 source 均返回 Ambiguous（即使内容完全相同）。
/// 输入不修改；成功时 retained + added == 输出消息数。
pub fn merge_chat_json(existing: &Value, incoming: &Value) -> Result<MergeResult, MergeError> {
    let (old_username, old) = read_messages(existing, "existing")?;
    let (new_username, new) = read_messages(incoming, "incoming")?;
    if old_username != new_username {
        return Err(MergeError::UsernameMismatch {
            existing: old_username.into(),
            incoming: new_username.into(),
        });
    }
    let mut report = MergeReport {
        retained: old.len(),
        ..MergeReport::default()
    };
    let mut groups: BTreeMap<&str, Vec<&Message<'_>>> = BTreeMap::new();
    for message in old.iter().chain(&new) {
        groups.entry(&message.id).or_default().push(message);
    }
    let conflicts: Vec<_> = groups
        .values()
        .filter(|group| group.len() > 1 && group.iter().any(|m| m.source.is_none()))
        .map(|group| Ambiguity {
            local_id: group[0].value["local_id"].clone(),
            locations: group.iter().map(|m| m.location.clone()).collect(),
        })
        .collect();
    if !conflicts.is_empty() {
        report.ambiguous = conflicts.len();
        return Err(MergeError::Ambiguous { report, conflicts });
    }

    // 保留已有重复，不丢失可能各自携带的转录或自定义字段。
    let mut seen: HashSet<(Option<&str>, &str)> =
        old.iter().map(|m| (m.source, m.id.as_str())).collect();
    let mut merged: Vec<&Message<'_>> = old.iter().collect();
    for message in &new {
        if seen.insert((message.source, message.id.as_str())) {
            merged.push(message);
            report.added += 1;
        } else {
            report.duplicates += 1;
        }
    }
    merged.sort_by_key(|m| m.timestamp);
    let mut document = existing.as_object().expect("validated object").clone();
    for (key, value) in incoming.as_object().expect("validated object") {
        document.entry(key.clone()).or_insert_with(|| value.clone());
    }
    document.insert(
        "messages".into(),
        Value::Array(merged.iter().map(|m| m.value.clone()).collect()),
    );
    Ok(MergeResult {
        document: Value::Object(document),
        report,
    })
}

#[cfg(test)]
#[path = "chat_archive_merge_tests.rs"]
mod tests;
