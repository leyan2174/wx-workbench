//! Shared error-shape assertion for hosts of the synthetic MCP account fixtures.
use serde_json::{json, Value};

pub(crate) fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}
