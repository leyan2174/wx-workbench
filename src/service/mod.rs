//! Shared daemon protocol and planning, independent of HTTP and CLI argument types.
pub mod client;
pub mod config_pin;
pub mod mcp;
pub mod message_filter;
pub mod operation_client;
pub mod operation_protocol;
pub mod operation_requests;
pub mod operations;
pub mod output;
pub mod plan;
pub mod protocol;
pub mod query_client;
pub mod settings;
pub mod time;
pub mod transport;
pub mod web;
