//! Fixed-account Web RPC contracts. No caller-selected key or output paths.
use crate::attachment::{AttachmentId, AttachmentKind};
use anyhow::{ensure, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const MAX_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// Local host startup settings, not a model- or HTTP-deserializable RPC request.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostSettings {
    pub port: u16,
    pub open: bool,
    pub image_cache_dir: Option<std::path::PathBuf>,
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
        limit: usize,
        offset: usize,
        since: Option<i64>,
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
