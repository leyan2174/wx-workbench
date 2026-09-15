#[path = "message_identity_adapters.rs"]
#[allow(dead_code)] // Partial hosts consume only the identity methods their queries need.
mod identities;
#[path = "../../src/adapters/wechat/messages/read/mod.rs"]
pub mod read;
pub mod messages {
    #[allow(unused_imports)] // Read-only embeddings need not consume identity lookup.
    pub use super::identities::{session_identity, sources};
    #[allow(unused_imports)] // Some hosts use only the legacy flattened read API.
    pub use super::read;
    pub use super::read::*;
}
#[path = "../../src/adapters/wechat/messages/inventory.rs"]
pub mod inventory;
pub mod wechat {
    #[allow(unused_imports)]
    // Standalone read tests need this alias; reply-only embeddings do not.
    pub use super::messages;
}
