// 独立编译真实 IPC 和协议文件，不修改主程序模块注册。
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/mcp/protocol.rs"]
pub mod protocol;
#[path = "../../../src/service/message_filter.rs"]
pub mod message_filter;
pub mod service {
    pub use crate::message_filter;
}
#[path = "../../../src/business/messages/mod.rs"]
#[allow(unfulfilled_lint_expectations)] // Public protocol-fixture types change production-only dead_code reachability.
#[allow(dead_code)] // Protocol tests project filters without constructing message references.
pub mod messages;
#[path = "../../../src/business/structured_message.rs"]
pub mod structured_message;
pub mod business {
    pub use crate::{messages, structured_message};
}
