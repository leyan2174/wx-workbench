#[path = "../../src/adapters/wechat/messages/read/mod.rs"]
pub mod messages;
pub use messages as read;
#[path = "../../src/adapters/wechat/messages/inventory.rs"]
pub mod inventory;
pub mod wechat {
    #[allow(unused_imports)] // Standalone read tests need this alias; reply-only embeddings do not.
    pub use super::messages;
}
