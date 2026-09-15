//! 引用回复解码：显式账号缓存、唯一消息定位，以及 type-57 结构化数据。
//! 供 daemon::query 接入；本模块不读取全局账号配置，也不调用传输层。
use super::{
    strict_message::{self, Resolution},
    DbCache, Names,
};
use crate::adapters::wechat::messages::reply::integer;
use crate::adapters::wechat::messages::reply_read::{self, Outcome};
use anyhow::Result;
use serde_json::{json, Value};
#[cfg(test)]
use std::collections::HashMap;

/// create_time=0 沿用旧契约，表示不按时间筛选，不是只查询时间戳为零的消息。
/// Ok 中 exit_code=0 表示成功，1 表示未找到或结构错误，2 表示联系人或消息歧义。
/// Err 表示缓存、分片或 SQLite 完整性错误，无法证明消息身份唯一。
/// names 必须与 db 来自同一账号；联系人歧义在任何数据库读取之前返回。
pub async fn q_decode_refer(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
) -> Result<Value> {
    let database = db.db_dir().to_path_buf();
    let display = chat.to_owned();
    let labels = names.map.clone();
    let outcome =
        match strict_message::with_resolved(
            db,
            names,
            chat,
            local_id,
            create_time,
            move |snapshot, raw| reply_read::decode(snapshot, raw, &database, &display, &labels),
        )
        .await?
        {
            Resolution::Found(outcome) => outcome,
            Resolution::ChatNotFound => return Ok(failure(1, "chat not found")),
            Resolution::AmbiguousChat => {
                return Ok(failure(2, "ambiguous chat; specify exact username"))
            }
            Resolution::MessageNotFound => return Ok(failure(1, "message not found")),
            Resolution::AmbiguousMessage => return Ok(failure(
                2,
                "ambiguous local_id; specify create_time (duplicate timestamps remain ambiguous)",
            )),
        };
    match outcome {
        Outcome::NotReply => Ok(failure(1, "not a reply: expected base_type=49")),
        Outcome::Found { parsed, source } => {
            #[derive(serde::Serialize)]
            struct ReplyResponse {
                exit_code: i32,
                text: String,
                refer: Value,
                #[serde(flatten)]
                source: reply_read::LegacySource,
            }
            let refer = project_refer(parsed);
            Ok(serde_json::to_value(ReplyResponse {
                exit_code: 0,
                text: render(&refer),
                refer,
                source,
            })?)
        }
        // 错误信息不得包含原始 XML、压缩字节或内层 CDN/密钥数据。
        Outcome::InvalidContent => Ok(failure(
            1,
            "invalid reply content: expected safe appmsg type=57 with refermsg",
        )),
    }
}

fn failure(code: i32, text: &str) -> Value {
    json!({"exit_code": code, "text": text})
}

#[cfg(test)]
fn parse_refer(
    body: &str,
    username: &str,
    display: &str,
    me: &str,
    names: &HashMap<String, String>,
) -> Result<Value> {
    let parsed =
        crate::adapters::wechat::messages::reply::parse_refer(body, username, display, me, names)?;
    Ok(project_refer(parsed))
}

fn project_refer(parsed: crate::adapters::wechat::messages::reply::ParsedReply) -> Value {
    json!({
        "reply_text": parsed.reply.text,
        "refer_sender": parsed.reply.sender_label,
        "refer_summary": parsed.reply.summary,
        "refer_type": parsed.kind,
        "refer_type_label": parsed.kind_label,
        "refer_svrid": parsed.server_id,
        "refer_createtime": parsed.created_at,
        "refer_fromusr": parsed.author,
        "refer_chatusr": parsed.conversation_author,
        "refer_displayname": parsed.display_name,
    })
}

fn render(refer: &Value) -> String {
    use chrono::{Local, TimeZone};
    let field = |key| refer[key].as_str().unwrap_or("");
    let reply = field("reply_text");
    let mut lines = vec![format!(
        "引用回复消息: {}",
        if reply.is_empty() {
            "(无回复正文)"
        } else {
            reply
        }
    )];
    for (key, title) in [
        ("refer_sender", "被引用消息发送者"),
        ("refer_displayname", "被引用消息显示名"),
        ("refer_fromusr", "被引用消息 from"),
        ("refer_chatusr", "被引用消息 chatusr (群内发送者 wxid)"),
    ] {
        if !field(key).is_empty() {
            lines.push(format!("  {title}: {}", field(key)));
        }
    }
    let kind = field("refer_type");
    let kind = if kind.is_empty() { "?" } else { kind };
    let label = field("refer_type_label");
    let kind = if label.is_empty() {
        format!("refer_type={kind}")
    } else {
        format!("{label} (refer_type={kind})")
    };
    lines.push(format!("  被引用消息类型: {kind}"));
    lines.push(format!("  被引用消息摘要: {}", field("refer_summary")));
    if let Some(ts) = integer(field("refer_createtime")).filter(|ts| *ts != 0) {
        let time = Local
            .timestamp_opt(ts, 0)
            .single()
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_else(|| format!("(无效 ts={})", field("refer_createtime")));
        lines.push(format!("  被引用消息创建时间: {time}"));
    }
    if !field("refer_svrid").is_empty() {
        lines.push(format!("  被引用消息 server_id: {}", field("refer_svrid")));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-refer/query_tests.rs"]
mod tests;
