//! Local type graph for the real strict media adapter; no copied parser or resolver.
#[path = "../../support/message_read_adapters.rs"]
mod message_sources;
pub use attachment_refs_contract::wechat_content as attachment_content;
pub use message_sources::{inventory, messages};
pub mod resource {
    pub use native_image_fixture::native_image::MessageIdentity;
}
#[path = "../../../src/adapters/wechat/media/strict_message.rs"]
#[allow(dead_code)] // Reference audits use only a subset of the strict media capture APIs.
pub(crate) mod strict_message;
pub mod wechat {
    pub use super::messages;
    // The real attachment query is embedded by the library's query_boundary tests.
    #[cfg(test)]
    pub mod media {
        pub(crate) use super::super::{attachment_content, strict_message};
    }
}
