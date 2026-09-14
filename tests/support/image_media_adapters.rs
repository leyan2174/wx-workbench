//! Production image discovery and metadata reads in one fixture-local type graph.
#[path = "../../src/adapters/wechat/messages/read.rs"]
pub mod message_read;
pub mod messages {
    pub use super::message_read as read;
    pub use read::*;
}
#[path = "../../src/adapters/wechat/media/mod.rs"]
pub mod media;
pub mod wechat {
    pub use super::{media, messages};
}
