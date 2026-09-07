//! 引用回复解码：显式账号缓存、唯一消息定位，以及 type-57 结构化数据。
//! 供 daemon::query 接入；本模块不读取全局账号配置，也不调用传输层。
use super::{
    strict_message::{self, Resolution},
    DbCache, Names,
};
use crate::message::export_content::{refer_label as label, refer_summary as summary};
use crate::message::xml;
use anyhow::{ensure, Context, Result};
use roxmltree::Node;
use serde_json::{json, Value};
use std::collections::HashMap;

const MAX_DECODED_BYTES: usize = 131_072;

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
    let message =
        match strict_message::locate(db, names, chat, local_id, create_time).await? {
            Resolution::Found(message) => message,
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
    let kind = message.kind;
    let base = if kind > u32::MAX as i64 {
        kind & 0xffff_ffff
    } else {
        kind
    };
    if base != 49 {
        return Ok(failure(1, "not a reply: expected base_type=49"));
    }
    let account = db
        .db_dir()
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let me = crate::message::identity::self_username(account, &names.map);
    let username = &message.username;
    let parsed = (|| -> Result<Value> {
        let bytes = message.bounded_decode(MAX_DECODED_BYTES)?;
        let text = String::from_utf8_lossy(&bytes);
        let body = if username.ends_with("@chatroom") {
            crate::message::split_group_content(&text).1
        } else {
            &text
        };
        parse_refer(body, username, chat, &me, &names.map)
    })();
    match parsed {
        Ok(refer) => Ok(
            json!({"exit_code": 0, "text": render(&refer), "refer": refer,
            "username": username, "local_id": message.local_id, "create_time": message.create_time, "source": message.source}),
        ),
        // 错误信息不得包含原始 XML、压缩字节或内层 CDN/密钥数据。
        Err(_) => Ok(failure(
            1,
            "invalid reply content: expected safe appmsg type=57 with refermsg",
        )),
    }
}

fn failure(code: i32, text: &str) -> Value {
    json!({"exit_code": code, "text": text})
}

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.has_tag_name(tag) && n.tag_name().namespace().is_none())
}

fn text<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> &'a str {
    child(node, tag).and_then(|n| n.text()).unwrap_or("")
}

fn appmsg<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.descendants()
        .skip(1)
        .find(|n| n.has_tag_name("appmsg") && n.tag_name().namespace().is_none())
}

// 沿用原生导出格式器支持的 ASCII 十进制整数子集，包括合法的下划线分隔。
fn integer(value: &str) -> Option<i64> {
    let value = value.trim();
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if digits.is_empty()
        || !digits
            .split('_')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    value.replace('_', "").parse().ok()
}

fn parse_refer(
    body: &str,
    username: &str,
    display: &str,
    me: &str,
    names: &HashMap<String, String>,
) -> Result<Value> {
    let doc = xml::parse(body).context("unsafe XML")?;
    let app = appmsg(doc.root_element()).context("missing appmsg")?;
    ensure!(integer(text(app, "type")) == Some(57), "not type 57");
    let refer = child(app, "refermsg").context("missing refermsg")?;
    let field = |key| xml::collapse(text(refer, key));
    let kind = field("type");
    let from = field("fromusr");
    let name = field("displayname");
    let sender = if username.ends_with("@chatroom") {
        if from.is_empty() {
            name.clone()
        } else if !me.is_empty() && from == me {
            "me".into()
        } else {
            names.get(&from).unwrap_or(&from).clone()
        }
    } else if !from.is_empty() {
        if from == username {
            display.into()
        } else if !me.is_empty() && from == me {
            "me".into()
        } else {
            names.get(&from).cloned().unwrap_or_else(|| {
                if name.is_empty() {
                    from.clone()
                } else {
                    name.clone()
                }
            })
        }
    } else if name == display {
        display.into()
    } else if !me.is_empty() && names.get(me).map(String::as_str).unwrap_or(me) == name {
        "me".into()
    } else {
        name.clone()
    };
    Ok(json!({"reply_text": xml::collapse(text(app, "title")),
        "refer_sender": sender, "refer_type": kind, "refer_type_label": label(&kind).unwrap_or(""),
        "refer_summary": summary(&kind, text(refer, "content")),
        "refer_svrid": field("svrid"), "refer_createtime": field("createtime"),
        "refer_fromusr": from, "refer_chatusr": field("chatusr"), "refer_displayname": name}))
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
