// 只加载实际身份类型和只读 resolver，不加载 decoder、音频、daemon 或网络服务。
#[path = "../../../src/attachment/attachment_id.rs"]
pub mod attachment_id;
pub use attachment_id::{AttachmentId, AttachmentKind};
#[path = "../../../src/attachment/resolver.rs"]
pub mod resolver;

#[cfg(test)]
mod probes;
#[path = "../../support/resource_media_adapters.rs"]
pub mod adapters;
