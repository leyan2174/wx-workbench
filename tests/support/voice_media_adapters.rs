//! Real metadata and voice adapters for hosts that do not project message bodies.
#[path = "../../src/adapters/wechat/messages/read.rs"]
pub mod message_read;
#[path = "../../src/adapters/wechat/media/voice.rs"]
pub mod voice;
#[path = "../../src/adapters/wechat/media/voice_catalog.rs"]
pub mod voice_catalog;

pub mod wechat {
    pub mod messages {
        pub use super::super::message_read as read;
        pub use read::*;
    }
    pub mod media {
        pub use super::super::{voice, voice_catalog};
    }
}
