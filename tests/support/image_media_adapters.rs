//! Production image discovery and metadata reads in one fixture-local type graph.
#[path = "../../src/adapters/wechat/messages/read/mod.rs"]
pub mod message_read;
pub use message_read as read;
#[path = "../../src/adapters/wechat/messages/inventory.rs"]
pub mod message_inventory;
#[path = "../../src/adapters/wechat/messages/probe.rs"]
pub mod message_probe;
pub mod messages {
    pub use super::message_probe as probe;
    pub use super::message_inventory as inventory;
    pub use super::message_read as read;
    pub use read::*;
}
#[path = "../../src/adapters/wechat/media/mod.rs"]
pub mod media;
#[path = "../../src/adapters/wechat/emoticons/mod.rs"]
pub mod emoticons;
#[path = "../../src/adapters/wechat/contacts/batch.rs"]
pub mod contact_batch;
pub mod wechat {
    pub mod contacts {
        pub use super::super::contact_batch as batch;
    }
    pub use super::{emoticons, media, messages};
}
