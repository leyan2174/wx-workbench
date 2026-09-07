// 独立编译真实 IPC 和协议文件，不修改主程序模块注册。
#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/mcp/protocol.rs"]
pub mod protocol;
