//! Production image discovery and metadata reads in one fixture-local type graph.
#[path = "../../src/adapters/wechat/messages/read/mod.rs"]
pub mod message_read;
pub mod messages {
    pub use super::message_read as read;
    pub use read::*;
}
#[path = "../../src/adapters/wechat/media/mod.rs"]
pub mod media;
#[path = "../../src/adapters/wechat/emoticons/mod.rs"]
pub mod emoticons;
pub mod wechat {
    pub use super::{emoticons, media, messages};
}
