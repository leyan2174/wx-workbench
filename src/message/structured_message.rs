//! Legacy rich JSON projection. Business metadata carries no wire-format rules.
use crate::business::structured_message::StructuredMessage;
use serde_json::{json, Value};

pub fn project(message: &StructuredMessage) -> Value {
    match message {
        StructuredMessage::Link {
            title,
            des,
            url,
            source,
        } => json!({"type": "link", "title": title, "des": des, "url": url, "source": source}),
        StructuredMessage::File {
            title,
            file_ext,
            file_size,
        } => json!({"type": "file", "title": title, "file_ext": file_ext, "file_size": file_size}),
        StructuredMessage::Miniapp { title, source, url } => {
            json!({"type": "miniapp", "title": title, "source": source, "url": url})
        }
        StructuredMessage::Channels { title } => json!({"type": "channels", "title": title}),
        StructuredMessage::Chatlog { title, des, items } => {
            json!({"type": "chatlog", "title": title, "des": des, "items": items})
        }
        StructuredMessage::Quote {
            title,
            ref_name,
            ref_content,
        } => {
            json!({"type": "quote", "title": title, "ref_name": ref_name, "ref_content": ref_content})
        }
        StructuredMessage::Transfer {
            title,
            status,
            raw_subtype,
            amount_text,
            memo,
        } => json!({"type": "transfer", "title": title,
                "direction": super::transfer::known_status_label(*status),
                "paysubtype": raw_subtype, "fee_desc": amount_text, "pay_memo": memo}),
        StructuredMessage::Voice { duration } => json!({"type": "voice", "duration": duration}),
        StructuredMessage::Video { duration } => json!({"type": "video", "duration": duration}),
        StructuredMessage::Emoji { emoji_url, md5 } => {
            json!({"type": "emoji", "emoji_url": emoji_url, "md5": md5})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::structured_message::TransferStatus;

    #[test]
    fn raw_diagnostic_code_does_not_determine_business_state() {
        let message = StructuredMessage::Transfer {
            title: "Payment".into(),
            status: TransferStatus::Unknown,
            raw_subtype: "3".into(),
            amount_text: "0.010 CNY".into(),
            memo: "memo".into(),
        };
        assert_eq!(
            project(&message),
            json!({"type": "transfer", "title": "Payment",
            "direction": "", "paysubtype": "3", "fee_desc": "0.010 CNY", "pay_memo": "memo"})
        );
    }
}
