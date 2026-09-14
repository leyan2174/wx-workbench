# 请求契约归属

`service::operation_requests` 是操作请求的唯一 serde 定义；`service::Operation` 引用这些类型，不引用 daemon 的 Args。`service::mcp` 直接拥有 `Call`、`HostSettings` 和 `VoiceSettings`，不再反向导出 daemon 类型。

`cli::operation_args` 仅承担 clap 参数解析。CLI 解析后用显式 `From` 转成同一份 service 请求，daemon 直接消费该请求，不增加第三套执行 DTO，也不通过 JSON 往返转换。枚举转换逐项匹配，字段转换保留路径、账号标识、授权位和显式后端设置。CLI 默认值与 serde 缺省行为是不同契约，不相互替代。

## 兼容边界

- 保留原有 Operation 标签、嵌套字段、serde 别名及 deny_unknown_fields；不改旧查询 Response 外形。
- clap 名称、别名、冲突和默认值留在 CLI；IPC 请求仍必须独立调用 validate_request，不能靠 CLI 校验保证安全。
- 上传授权不自动补齐，不读取凭据来完成参数转换；MCP 主机设置仍来自宿主启动参数，不能由 tools/call 注入。
- 日期解析迁入 service::time，保留本地时区、模糊时间拒绝和结束日期包含整天的既有行为；本次不改授权窗口或账号绑定。
- 纯参数校验位于请求模块，复用既有工具层的纯校验函数。文件读取、模型构建及实际执行仍留在原执行模块；本次不宣称全部业务实现已移出 daemon。

## 回归与接线

`cli::request_conversion_tests` 检查 CLI 到请求的默认值、ASR 旧别名、显式上传授权、MCP 字段和非法反序列化请求；既有 Operation wire 测试继续检查序列化。

`tests/architecture_contracts.rs` 用 syn AST 检查公共契约不依赖 daemon/cli/clap，并检查 business 不依赖 daemon、CLI、clap、rusqlite、adapters、wechat_data 或 serde_json::Value。注释不参与判断；重命名导入和派生宏会参与判断。

独立 ASR/MCP 安全宿主通过 tests/support 中的小型接线模块引用真实请求与解析源码。每个宿主只实例化一份契约；依赖其他宿主执行实现时复用其 service 契约，避免同名不同类型。生产代码使用正常模块声明，没有为 fixture 增加生产 #[path] 垫片。

`entry_architecture` 对 daemon/service 全目录使用 syn 检查生产依赖，仅跳过明确的 `#[cfg(test)]` 语法项，不按文件名或模块名豁免。执行集成测试可解析真实 CLI 参数再转换请求；纯 CLI 别名断言归 CLI 测试。额外反例确保分组、重命名导入以及非 test-only 的 tests 模块仍受检查。
