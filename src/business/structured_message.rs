//! Read-only message content, independent of storage and protocol transports.

/// A missing preview does not mean the original message is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentIssue {
    UnsupportedKind,
    InputTooLarge,
    MalformedContent,
    NoSafePreview,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StructuredMessage {
    Link {
        title: String,
        des: String,
        url: String,
        source: String,
    },
    File {
        title: String,
        file_ext: String,
        file_size: u64,
    },
    Miniapp {
        title: String,
        source: String,
        url: String,
    },
    Channels {
        title: String,
    },
    Chatlog {
        title: String,
        des: String,
        items: Vec<ChatItem>,
    },
    Quote {
        title: String,
        ref_name: String,
        ref_content: String,
    },
    Transfer {
        title: String,
        direction: String,
        paysubtype: String,
        fee_desc: String,
        pay_memo: String,
    },
    Voice {
        duration: f64,
    },
    Video {
        duration: u64,
    },
    Emoji {
        emoji_url: String,
        md5: String,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ChatItem {
    pub name: String,
    pub text: String,
}
