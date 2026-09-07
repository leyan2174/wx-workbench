//! 聊天附件提取链路（图片 / 视频 / 语音 / 文件本体的本地解码）
//!
//! 整条链：
//!   message_N.db (Msg_<md5>) → message_resource.db (ChatName2Id + MessageResourceInfo)
//!     → packed_info protobuf md5 提取 → xwechat_files/<wxid>/msg/attach/.../Img/<md5>[_t|_h].dat
//!     → magic 分发 (legacy XOR / V1 fixed-AES / V2 AES+XOR) → 写出实际图片
//!
//! 模块切分：
//! - `attachment_id`：跨 IPC / CLI 的不透明 ID（base64url(json)）
//! - `resolver`：从 `attachment_id` 反查 message_resource.db，定位本地 .dat
//! - `decoder`：根据文件 magic 分发到具体解码器（V1 / V2 等）
//! - `image_key`：Windows V2 image AES key 提取
//!
//! `native_image` 提供显式密钥、严格消息资源关联和无覆盖输出，不自动取钥。

// 兼容提取 API 尚未全部收口；最终精简时逐项审查未调用接口。
#![allow(dead_code)]

pub mod attachment_id;
pub mod decoder;
pub mod image_key;
pub mod image_metadata;
pub(crate) mod local_files;
pub mod native_image;
pub mod resolver;

pub use attachment_id::{AttachmentId, AttachmentKind};
