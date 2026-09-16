#[path = "../../src/business/attachment_content.rs"]
pub mod attachment_content;
#[path = "../../src/business/emoticons.rs"]
#[allow(unfulfilled_lint_expectations)] // Public fixture APIs change production-only dead_code reachability.
#[allow(dead_code)] // Voice/read-only fixtures do not construct catalog media references.
pub mod emoticons;
#[path = "../../src/business/media.rs"]
#[allow(dead_code)] // Shared domain: voice-only targets do not construct attachment identities.
#[allow(unfulfilled_lint_expectations)]
// Public/partial embedding differs from private production reachability.
pub mod media;
#[path = "../../src/business/messages/mod.rs"]
#[allow(unfulfilled_lint_expectations)]
// Message variants are public fixture API, unlike the production module.
pub mod messages;
#[path = "../../src/business/structured_message.rs"]
pub mod structured_message;
#[path = "../../src/business/voice/mod.rs"]
#[allow(dead_code, unfulfilled_lint_expectations)]
// Public slice fixtures may omit catalog evidence consumers; production expects remain active.
pub mod voice;
#[path = "../../src/business/voice_export.rs"]
pub mod voice_export;
