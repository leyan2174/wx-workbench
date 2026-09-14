//! Run the standalone history compatibility checks against Cargo's real wx binary.
#![cfg(windows)]

#[path = "fixtures/mcp-history-compat/runtime.rs"]
mod history;

use history::safe_failure;
