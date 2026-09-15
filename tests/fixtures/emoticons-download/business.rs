//! Real sibling business modules, preserving their production relative imports.
#[path = "../../../src/business/emoticons.rs"]
#[allow(unfulfilled_lint_expectations)] // Public fixture APIs change production-only dead_code reachability.
pub mod emoticons;
#[path = "../../../src/business/media.rs"]
#[allow(dead_code)] // Emoticon catalog references do not construct message attachment identities.
#[allow(unfulfilled_lint_expectations)] // Public/partial embedding differs from private production reachability.
pub mod media;
#[path = "../../../src/business/messages/mod.rs"]
#[allow(unfulfilled_lint_expectations)] // Message variants remain public for sibling adapter tests.
#[allow(dead_code)] // Download tests do not validate message references.
pub mod messages;
#[path = "../../../src/business/structured_message.rs"]
pub mod structured_message;
