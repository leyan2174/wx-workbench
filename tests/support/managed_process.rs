//! The production managed executor, including its synthetic lifecycle tests.
#[path = "../../src/windows_process/managed.rs"]
#[allow(dead_code)]
// ASR/download fixtures do not invoke account-capture Job APIs; retain lifecycle tests.
pub mod managed;
