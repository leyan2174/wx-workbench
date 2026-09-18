# 请求契约归属

`service::operation_requests` 是操作请求的唯一 serde 定义；`service::Operation` 引用这些类型，不引用 daemon 的 Args。`service::mcp` 直接拥有 `Call` 和 `HostSettings`，不反向导出 daemon 类型。

`cli::operation_args` 仅承担 clap 参数解析。CLI 解析后用显式 `From` 转成同一份 service 请求，daemon 直接消费该请求，不增加第三套执行 DTO，也不通过 JSON 往返转换。枚举转换逐项匹配，字段转换保留路径、账号标识、授权位。CLI 默认值与 serde 缺省行为是不同契约，不相互替代。

## 兼容边界

- 保留原有 Operation 标签、嵌套字段、serde 别名及 deny_unknown_fields；不改旧查询 Response 外形。
- clap 名称、别名、冲突和默认值留在 CLI；IPC 请求仍必须独立调用 validate_request，不能靠 CLI 校验保证安全。
- 媒体下载授权不自动补齐；MCP 主机设置仍来自宿主启动参数，不能由 tools/call 注入。
- CLI 日期解析位于 `service::time`，使用本地时区，拒绝模糊时间，结束日期包含整天；MCP/HTTP 查询接收 Unix 秒，不套用 CLI 日期字符串语义。授权窗口和账号绑定独立校验。
- 请求模块执行纯参数校验，不读取文件或执行工作流；这些副作用由对应执行模块负责。

## 回归与接线

只读查询共用 `ipc::Request`。CLI 的 `query_details` 负责标签、结构化详情和语音目录的 clap 参数与映射，再调用共享查询客户端；MCP 通过工具 schema 和固定账号白名单；HTTP 经 `web::read_queries`、`service::web::Call::validate_read` 和 daemon 的显式映射进入查询分发。不增加任意 Request/Operation 穿透入口。各端参数并非完全相同，详见[查询协议](query-protocol.md)与[HTTP API](http-api.md)。

`cli::request_conversion_tests` 检查 CLI 到请求的默认值、显式授权、MCP 字段和非法反序列化请求；既有 Operation wire 测试继续检查序列化。

`tests/architecture_contracts.rs` 用 syn AST 检查公共契约不依赖 daemon/cli/clap，并检查 business 不依赖 daemon、CLI、clap、rusqlite、adapters、wechat_data 或 serde_json::Value。注释不参与判断；重命名导入和派生宏会参与判断。

独立 MCP 安全宿主通过 tests/support 中的小型接线模块引用真实请求与解析源码。每个宿主只实例化一份契约；依赖其他宿主执行实现时复用其 service 契约，避免同名不同类型。生产代码使用正常模块声明，没有为 fixture 增加生产 #[path] 垫片。

`entry_architecture` 对 daemon/service 全目录使用 syn 检查生产依赖，仅跳过明确的 `#[cfg(test)]` 语法项，不按文件名或模块名豁免。执行集成测试可解析真实 CLI 参数再转换请求；纯 CLI 别名断言归 CLI 测试。额外反例确保分组、重命名导入以及非 test-only 的 tests 模块仍受检查。
