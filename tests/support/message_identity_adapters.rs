//! Real lightweight identity adapters for partial message-read test hosts.
pub use super::read;
#[path = "../../src/adapters/wechat/messages/probe.rs"]
pub mod probe;
#[path = "../../src/adapters/wechat/messages/session_identity.rs"]
pub mod session_identity;
#[path = "../../src/adapters/wechat/messages/sources.rs"]
pub mod sources;
