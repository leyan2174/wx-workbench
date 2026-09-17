# MCP 命令入口测试

夹具验证 stdio 协议适配、参数校验和宿主设置。初始化与 tools/list 不读取账号或密钥，不创建媒体输出目录；首次业务请求才固定显式账号配置。

CLI 封送宿主参数并通过认证 Call::Mcp 交给 daemon。工具参数不能提供账号路径、后端凭据、输出目录或上传授权。退出、EOF、daemon 重启和期限耗尽不得绕过会话身份或重新授权。

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-cli/Cargo.toml -- --nocapture
```

受控传输与合成程序不能代替真实 daemon 测试。工具字段、错误和帧预算以[MCP 协议](../../../src/mcp/PROTOCOL.md)为准。
