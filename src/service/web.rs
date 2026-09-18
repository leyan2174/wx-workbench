//! Fixed-account Web RPC contracts. No caller-selected key or output paths.
use crate::attachment::{AttachmentId, AttachmentKind};
use anyhow::{ensure, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const MAX_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

pub const QUERY_AMBIGUOUS_CODE: &str = "query_ambiguous";
pub const QUERY_AMBIGUOUS_MESSAGE: &str =
    "Query identity is ambiguous; specify an exact conversation and message";

#[derive(Debug)]
pub struct QueryAmbiguity;
impl std::fmt::Display for QueryAmbiguity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(QUERY_AMBIGUOUS_MESSAGE)
    }
}
impl std::error::Error for QueryAmbiguity {}

fn query_limit() -> usize {
    200
}

fn text(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
        "invalid query text"
    );
    Ok(())
}

fn time_range(since: Option<i64>, until: Option<i64>) -> Result<()> {
    ensure!(
        since.is_none_or(|time| time >= 0) && until.is_none_or(|time| time >= 0),
        "invalid timestamp"
    );
    ensure!(
        since.zip(until).is_none_or(|(since, until)| since <= until),
        "invalid time range"
    );
    Ok(())
}

impl Call {
    /// Shared HTTP and daemon validation for the explicit read-only query calls.
    pub fn validate_read(&self) -> Result<()> {
        match self {
            Self::History {
                chat,
                limit,
                offset,
                since,
                until,
                msg_type,
                msg_types,
                ..
            } => {
                text(chat)?;
                ensure!((1..=2000).contains(limit), "invalid query limit");
                ensure!(*offset <= 1_000_000, "invalid query offset");
                time_range(*since, *until)?;
                ensure!(
                    msg_type.is_none_or(|kind| kind >= 0),
                    "invalid message type"
                );
                if let Some(types) = msg_types {
                    ensure!(
                        msg_type.is_none()
                            && !types.is_empty()
                            && types.len() <= 64
                            && types.iter().all(|kind| *kind >= 0),
                        "invalid message types"
                    );
                }
            }
            Self::Search {
                keyword,
                chats,
                limit,
                since,
                until,
                msg_type,
                ..
            } => {
                text(keyword)?;
                ensure!((1..=2000).contains(limit), "invalid query limit");
                time_range(*since, *until)?;
                ensure!(
                    msg_type.is_none_or(|kind| kind >= 0),
                    "invalid message type"
                );
                if let Some(chats) = chats {
                    ensure!(!chats.is_empty() && chats.len() <= 100, "invalid chats");
                    for chat in chats {
                        text(chat)?;
                    }
                }
            }
            Self::Unread { limit, filter, .. } => {
                ensure!((1..=2000).contains(limit), "invalid query limit");
                if let Some(filters) = filter {
                    ensure!(
                        !filters.is_empty()
                            && filters.len() <= 5
                            && filters.iter().all(|filter| matches!(
                                filter.as_str(),
                                "private" | "group" | "official" | "folded" | "all"
                            )),
                        "invalid unread filter"
                    );
                }
            }
            Self::Members { chat, .. } => {
                text(chat)?;
            }
            Self::Stats {
                chat, since, until, ..
            } => {
                text(chat)?;
                time_range(*since, *until)?;
            }
            Self::Favorites {
                limit,
                fav_type,
                query,
                ..
            } => {
                ensure!((1..=2000).contains(limit), "invalid query limit");
                if let Some(value) = query {
                    text(value)?;
                }
                ensure!(
                    fav_type.is_none_or(|kind| kind >= 0),
                    "invalid favorite type"
                );
            }
            Self::BizArticles {
                limit,
                account,
                since,
                until,
                ..
            } => {
                ensure!((1..=2000).contains(limit), "invalid query limit");
                if let Some(value) = account {
                    text(value)?;
                }
                time_range(*since, *until)?;
            }
            Self::SnsFeed {
                limit,
                since,
                until,
                user,
                ..
            } => {
                ensure!((1..=2000).contains(limit), "invalid query limit");
                if let Some(value) = user {
                    text(value)?;
                }
                time_range(*since, *until)?;
            }
            Self::SnsSearch {
                keyword,
                limit,
                since,
                until,
                user,
                ..
            } => {
                text(keyword)?;
                ensure!((1..=2000).contains(limit), "invalid query limit");
                if let Some(value) = user {
                    text(value)?;
                }
                time_range(*since, *until)?;
            }
            Self::SnsNotifications {
                limit,
                since,
                until,
                ..
            } => {
                ensure!((1..=2000).contains(limit), "invalid query limit");
                time_range(*since, *until)?;
            }
            Self::VoiceMessages {
                chat,
                limit,
                offset,
                since,
                until,
                ..
            } => {
                text(chat)?;
                ensure!((1..=500).contains(limit), "invalid voice query limit");
                ensure!(*offset <= 1_000_000, "invalid query offset");
                time_range(*since, *until)?;
            }
            Self::DecodeTransfer {
                chat,
                local_id,
                create_time,
                ..
            } => {
                text(chat)?;
                ensure!(
                    *local_id > 0 && *local_id < i64::MAX,
                    "exact message identity required"
                );
                ensure!(
                    *create_time > 0 && *create_time < i64::MAX,
                    "exact message identity required"
                );
            }
            Self::DecodeLocation {
                chat,
                local_id,
                create_time,
                ..
            } => {
                text(chat)?;
                ensure!(
                    *local_id > 0 && *local_id < i64::MAX,
                    "exact message identity required"
                );
                ensure!(
                    *create_time > 0 && *create_time < i64::MAX,
                    "exact message identity required"
                );
            }
            Self::DecodeRefer {
                chat,
                local_id,
                create_time,
                ..
            } => {
                text(chat)?;
                ensure!(
                    *local_id > 0 && *local_id < i64::MAX,
                    "exact message identity required"
                );
                ensure!(
                    *create_time > 0 && *create_time < i64::MAX,
                    "exact message identity required"
                );
            }
            Self::DecodeFileMessage {
                chat,
                local_id,
                create_time,
                ..
            } => {
                text(chat)?;
                ensure!(
                    *local_id > 0 && *local_id < i64::MAX,
                    "exact message identity required"
                );
                ensure!(
                    *create_time > 0 && *create_time < i64::MAX,
                    "exact message identity required"
                );
            }
            Self::DecodeRecordItem {
                chat,
                local_id,
                create_time,
                item_index,
                ..
            } => {
                text(chat)?;
                ensure!(
                    *local_id > 0 && *local_id < i64::MAX,
                    "exact message identity required"
                );
                ensure!(
                    *create_time > 0 && *create_time < i64::MAX,
                    "exact message identity required"
                );
                ensure!(*item_index >= 0, "invalid record item index");
            }
            _ => anyhow::bail!("not a read query"),
        }
        Ok(())
    }
}

/// Local host startup settings, not a model- or HTTP-deserializable RPC request.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostSettings {
    pub port: u16,
    pub open: bool,
    pub image_cache_dir: Option<std::path::PathBuf>,
    pub allow_plan_scan: bool,
    pub allow_media_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Call {
    DecodeImage {
        encoded: String,
        source: String,
    },
    PreviewImage {
        encoded: String,
    },
    Images {
        chat: String,
        limit: usize,
        offset: usize,
        since: Option<i64>,
    },
    History {
        chat: String,
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        msg_type: Option<i64>,
        #[serde(default)]
        msg_types: Option<Vec<i64>>,
        #[serde(default)]
        oldest_first: bool,
        #[serde(default)]
        with_meta: bool,
        #[serde(default)]
        debug_source: bool,
    },
    Search {
        keyword: String,
        #[serde(default)]
        chats: Option<Vec<String>>,
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        msg_type: Option<i64>,
        #[serde(default)]
        with_meta: bool,
        #[serde(default)]
        debug_source: bool,
    },
    Unread {
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        filter: Option<Vec<String>>,
        #[serde(default)]
        with_meta: bool,
        #[serde(default)]
        debug_source: bool,
    },
    Members {
        chat: String,
    },
    Stats {
        chat: String,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        with_meta: bool,
        #[serde(default)]
        debug_source: bool,
    },
    Favorites {
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        fav_type: Option<i64>,
        #[serde(default)]
        query: Option<String>,
    },
    BizArticles {
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        account: Option<String>,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        unread: bool,
    },
    SnsFeed {
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        user: Option<String>,
    },
    SnsSearch {
        keyword: String,
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        user: Option<String>,
    },
    SnsNotifications {
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
        #[serde(default)]
        include_read: bool,
    },
    VoiceMessages {
        chat: String,
        #[serde(default = "query_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        until: Option<i64>,
    },
    DecodeTransfer {
        chat: String,
        local_id: i64,
        create_time: i64,
    },
    DecodeLocation {
        chat: String,
        local_id: i64,
        create_time: i64,
    },
    DecodeRefer {
        chat: String,
        local_id: i64,
        create_time: i64,
    },
    DecodeFileMessage {
        chat: String,
        local_id: i64,
        create_time: i64,
    },
    DecodeRecordItem {
        chat: String,
        local_id: i64,
        create_time: i64,
        item_index: i64,
    },
    Tags {
        name: Option<String>,
    },
    MonitorOpen {},
    MonitorEvents {
        epoch: String,
        after: u64,
    },
    MonitorHistory {
        session: String,
        limit: usize,
        offset: usize,
        since: Option<i64>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Failure {
    InvalidIdentity,
    Busy,
    Ambiguous,
    Unavailable,
    UnsupportedFormat,
    DecodeFailed,
}
impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "invalid_identity",
            Self::Busy => "busy",
            Self::Ambiguous => "ambiguous",
            Self::Unavailable => "unavailable",
            Self::UnsupportedFormat => "unsupported_format",
            Self::DecodeFailed => "decode_failed",
        }
    }
    pub fn http_status(self) -> u16 {
        match self {
            Self::InvalidIdentity => 400,
            Self::Busy => 429,
            Self::Ambiguous => 409,
            Self::Unavailable => 404,
            Self::UnsupportedFormat => 415,
            Self::DecodeFailed => 503,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "图片缺少精确消息身份或逻辑分片",
            Self::Busy => "图片解码繁忙，请稍后重试",
            Self::Ambiguous => "图片消息身份有歧义，未选择任何候选",
            Self::Unavailable => "图片消息尚不可用或服务正在关闭",
            Self::UnsupportedFormat => "该图片格式无法原生预览，未启动外部转换器",
            Self::DecodeFailed => "图片解码或校验失败，请检查本地图像密钥及资源；未自动取钥或下载",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub data: String,
    pub content_type: String,
}
impl Image {
    pub fn new(bytes: Vec<u8>, content_type: &str) -> Result<Self> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
            "image size exceeded"
        );
        Ok(Self {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            content_type: content_type.into(),
        })
    }
    pub fn into_bytes(self) -> Result<(Vec<u8>, &'static str)> {
        let mime = match self.content_type.as_str() {
            "image/jpeg" => "image/jpeg",
            "image/png" => "image/png",
            "image/gif" => "image/gif",
            "image/webp" => "image/webp",
            "image/bmp" => "image/bmp",
            _ => anyhow::bail!("invalid image MIME"),
        };
        ensure!(
            self.data.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4,
            "image frame too large"
        );
        let bytes = base64::engine::general_purpose::STANDARD.decode(self.data)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
            "image size exceeded"
        );
        Ok((bytes, mime))
    }
}
pub fn identity(encoded: &str) -> Result<AttachmentId> {
    ensure!(encoded.len() <= 2048, "附件标识过长");
    let id = AttachmentId::decode(encoded)?;
    ensure!(
        id.kind == AttachmentKind::Image
            && id.db.is_none()
            && id.local_id > 0
            && id.create_time >= 0
            && id.create_time < i64::MAX
            && !id.chat.is_empty()
            && id.chat.len() <= 256
            && !id.chat.chars().any(char::is_control),
        "图片标识无效"
    );
    ensure!(id.encode()? == encoded, "图片标识不是规范编码");
    Ok(id)
}

pub fn valid_source(source: &str) -> bool {
    source.len() <= 64
        && source
            .strip_prefix("message/message_")
            .and_then(|s| s.strip_suffix(".db"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn exact_identity(encoded: &str) -> Result<AttachmentId> {
    let id = identity(encoded)?;
    // 严格 IPC 的零时间是通配条件，Web 不允许用它冒充精确身份。
    ensure!(id.create_time > 0, "image timestamp must be exact");
    Ok(id)
}

#[cfg(test)]
pub(crate) fn read_query_cases() -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    vec![
        (
            "/api/history",
            json!({"op":"history","chat":"wxid_http_fixture","limit":7,"offset":3,"since":100,"until":200,"msg_type":null,"msg_types":[1,3],"oldest_first":true,"with_meta":true,"debug_source":true}),
        ),
        (
            "/api/search",
            json!({"op":"search","keyword":"needle","chats":["wxid_a","wxid_b"],"limit":7,"since":100,"until":200,"msg_type":34,"with_meta":true,"debug_source":true}),
        ),
        (
            "/api/unread",
            json!({"op":"unread","limit":7,"filter":["private","group"],"with_meta":true,"debug_source":true}),
        ),
        (
            "/api/members",
            json!({"op":"members","chat":"wxid_http_fixture"}),
        ),
        (
            "/api/stats",
            json!({"op":"stats","chat":"wxid_http_fixture","since":100,"until":200,"with_meta":true,"debug_source":true}),
        ),
        (
            "/api/favorites",
            json!({"op":"favorites","limit":7,"fav_type":5,"query":"needle"}),
        ),
        (
            "/api/articles",
            json!({"op":"biz_articles","limit":7,"account":"needle","since":100,"until":200,"unread":true}),
        ),
        (
            "/api/sns-feed",
            json!({"op":"sns_feed","limit":7,"since":100,"until":200,"user":"needle"}),
        ),
        (
            "/api/sns-search",
            json!({"op":"sns_search","keyword":"needle","limit":7,"since":100,"until":200,"user":"needle"}),
        ),
        (
            "/api/sns-notifications",
            json!({"op":"sns_notifications","limit":7,"since":100,"until":200,"include_read":true}),
        ),
        (
            "/api/voice-messages",
            json!({"op":"voice_messages","chat":"wxid_http_fixture","limit":7,"offset":3,"since":100,"until":200}),
        ),
        (
            "/api/decode-transfer",
            json!({"op":"decode_transfer","chat":"wxid_http_fixture","local_id":71,"create_time":1700000123}),
        ),
        (
            "/api/decode-location",
            json!({"op":"decode_location","chat":"wxid_http_fixture","local_id":71,"create_time":1700000123}),
        ),
        (
            "/api/decode-refer",
            json!({"op":"decode_refer","chat":"wxid_http_fixture","local_id":71,"create_time":1700000123}),
        ),
        (
            "/api/decode-file-message",
            json!({"op":"decode_file_message","chat":"wxid_http_fixture","local_id":71,"create_time":1700000123}),
        ),
        (
            "/api/decode-record-item",
            json!({"op":"decode_record_item","chat":"wxid_http_fixture","local_id":71,"create_time":1700000123,"item_index":2}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rpc_never_accepts_caller_selected_business_paths() {
        let valid =
            json!({"op":"decode_image","encoded":"synthetic","source":"message/message_0.db"});
        assert!(serde_json::from_value::<Call>(valid.clone()).is_ok());
        for field in [
            "image_key_file",
            "output_root",
            "runtime_id",
            "config_path",
            "cache_dir",
        ] {
            let mut invalid = valid.clone();
            invalid[field] = json!("arbitrary");
            assert!(serde_json::from_value::<Call>(invalid).is_err());
        }
    }

    #[test]
    fn wire_images_are_bounded_and_have_only_bitmap_mime_types() -> Result<()> {
        let image = Image::new(vec![1, 2, 3], "image/png")?;
        assert_eq!(image.into_bytes()?, (vec![1, 2, 3], "image/png"));
        for mime in ["text/html", "image/svg+xml", "image/png\r\nInjected: true"] {
            assert!(Image {
                data: "AQID".into(),
                content_type: mime.into()
            }
            .into_bytes()
            .is_err());
        }
        for data in ["", "not base64"] {
            assert!(Image {
                data: data.into(),
                content_type: "image/png".into()
            }
            .into_bytes()
            .is_err());
        }
        assert!(Image::new(Vec::new(), "image/png").is_err());
        Ok(())
    }
}
