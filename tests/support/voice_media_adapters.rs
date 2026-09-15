//! Real metadata and voice adapters for hosts that do not project message bodies.
#[path = "../../src/adapters/wechat/contacts/batch.rs"]
pub mod contact_batch;
#[path = "../../src/adapters/wechat/messages/read/mod.rs"]
pub mod message_read;
#[path = "../../src/adapters/wechat/media/voice.rs"]
pub mod voice;
#[path = "../../src/adapters/wechat/media/voice_catalog.rs"]
pub mod voice_catalog;
#[path = "../../src/adapters/wechat/media/voice_export.rs"]
pub mod voice_export;

pub mod wechat {
    pub mod contacts {
        pub use super::super::contact_batch as batch;
    }
    pub mod messages {
        pub use super::super::message_read as read;
        pub use read::*;
    }
    pub mod media {
        pub use super::super::{voice, voice_catalog, voice_export};
    }
}
