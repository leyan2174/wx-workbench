//! 旧聊天导出的紧凑 JSON 契约。读取、发送者识别及内容解析由调用层负责。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Deserialize, Serialize)]
pub struct Target {
    pub username: String,
    pub chat: String,
    pub is_group: bool,
}

#[derive(Debug, Serialize)]
pub struct Message {
    pub local_id: i64,
    pub timestamp: Option<i64>,
    pub sender: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(flatten)]
    extras: Map<String, Value>,
}

impl Message {
    pub fn new(
        local_id: i64,
        local_type: i64,
        timestamp: Option<i64>,
        sender: String,
        content: Option<String>,
        mut extras: Map<String, Value>,
    ) -> anyhow::Result<Self> {
        // 扩展字段可以细化类型，但不能覆盖用于定位和证据归属的主字段。
        for key in ["local_id", "timestamp", "sender", "content"] {
            anyhow::ensure!(!extras.contains_key(key), "导出扩展字段不能覆盖 {key}");
        }
        let override_kind = extras.remove("type");
        let kind = match override_kind {
            Some(Value::String(value)) if !value.is_empty() => value,
            None | Some(Value::Null) => type_name(local_type),
            Some(Value::String(_)) => type_name(local_type),
            _ => anyhow::bail!("导出消息类型必须为字符串"),
        };
        Ok(Self {
            local_id,
            timestamp,
            sender,
            kind: (kind != "text").then_some(kind),
            content,
            extras,
        })
    }
}

fn type_name(local_type: i64) -> String {
    match local_type as u64 & 0xffff_ffff {
        1 => "text",
        3 => "image",
        34 => "voice",
        42 => "contact_card",
        43 => "video",
        47 => "sticker",
        48 => "location",
        49 => "link_or_file",
        50 => "call",
        10000 => "system",
        10002 => "recall",
        _ => return format!("type_{local_type}"),
    }
    .into()
}

#[derive(Debug, Serialize)]
pub struct Chat {
    pub chat: String,
    pub username: String,
    pub exported_at: String,
    #[serde(skip_serializing_if = "is_false")]
    pub is_group: bool,
    pub messages: Vec<Message>,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Chat {
    /// 同时间戳保留输入顺序；缺失时间按旧导出规则排在时间零处。
    pub fn sort_chronologically(&mut self) {
        self.messages
            .sort_by_key(|message| message.timestamp.unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn omits_defaults_but_preserves_empty_content_and_null_time() {
        let message = Message::new(1, 1, None, "".into(), Some("".into()), Map::new()).unwrap();
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({"local_id":1,"timestamp":null,"sender":"","content":""})
        );
        let voice =
            Message::new(2, (1 << 32) | 34, Some(100), "me".into(), None, Map::new()).unwrap();
        assert_eq!(
            serde_json::to_value(voice).unwrap(),
            json!({"local_id":2,"timestamp":100,"sender":"me","type":"voice"})
        );
    }

    #[test]
    fn transfer_override_and_unknown_types_remain_explicit() {
        let extras = json!({"type":"transfer","transfer":{"fee_desc":"¥0.01"}})
            .as_object()
            .unwrap()
            .clone();
        let message = Message::new(1, 49, Some(0), "me".into(), None, extras).unwrap();
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value["type"], "transfer");
        assert_eq!(value["transfer"]["fee_desc"], "¥0.01");
        assert_eq!(type_name((1 << 32) | 99), "type_4294967395");
        for key in ["local_id", "timestamp", "sender", "content"] {
            let mut extras = Map::new();
            extras.insert(key.into(), Value::Null);
            assert!(Message::new(1, 1, None, "".into(), None, extras).is_err());
        }
    }

    #[test]
    fn document_sorts_stably_and_only_marks_groups() {
        let mut chat = Chat {
            chat: "示例".into(),
            username: "demo".into(),
            exported_at: "2026-01-01 00:00:00".into(),
            is_group: false,
            messages: vec![],
        };
        for (id, time) in [(1, Some(100)), (2, None), (3, Some(100))] {
            chat.messages
                .push(Message::new(id, 1, time, "".into(), None, Map::new()).unwrap());
        }
        chat.sort_chronologically();
        assert_eq!(
            chat.messages.iter().map(|m| m.local_id).collect::<Vec<_>>(),
            [2, 1, 3]
        );
        assert!(serde_json::to_value(&chat)
            .unwrap()
            .get("is_group")
            .is_none());
        chat.is_group = true;
        assert_eq!(serde_json::to_value(&chat).unwrap()["is_group"], true);
    }
}
