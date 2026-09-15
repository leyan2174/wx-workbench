//! Strict quoted-message decoding. Raw diagnostic fields remain adapter-owned.
use crate::adapters::wechat::messages::export_content::{
    refer_label as label, refer_summary as summary,
};
use crate::business::messages::reply::Reply;
use crate::message::xml;
use anyhow::{ensure, Context, Result};
use roxmltree::Node;
use std::collections::HashMap;

pub struct ParsedReply {
    pub reply: Reply,
    pub kind: String,
    pub kind_label: String,
    pub server_id: String,
    pub created_at: String,
    pub author: String,
    pub conversation_author: String,
    pub display_name: String,
}

#[cfg(test)]
#[path = "../../../../tests/fixtures/mcp-refer/typed_tests.rs"]
mod typed_tests;

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
pub(crate) fn integer(value: &str) -> Option<i64> {
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

pub fn parse_refer(
    body: &str,
    username: &str,
    display: &str,
    me: &str,
    names: &HashMap<String, String>,
) -> Result<ParsedReply> {
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
    Ok(ParsedReply {
        reply: Reply {
            text: xml::collapse(text(app, "title")),
            sender_label: sender,
            summary: summary(&kind, text(refer, "content")),
        },
        kind_label: label(&kind).unwrap_or("").into(),
        kind,
        server_id: field("svrid"),
        created_at: field("createtime"),
        author: from,
        conversation_author: field("chatusr"),
        display_name: name,
    })
}
