# 真实 MCP 语音验收


入口：`tests/mcp_voice_runtime.rs`（Windows）。复用相邻只读夹具的合成账号、
数据库加密、daemon、命名管道和 MCP 进程管理，不替代生产关联或解码器。

- 媒体 ID `700` 通过 server ID 关联到消息 ID `7`，另有媒体 `701`。
- A/B 账号使用相同身份编号、不同长度的有效静音 SILK，比较实际 WAV 字节。
- `artifacts.rs` 独立构造预期 PCM WAV 和 prepared-audio wire oracle。
- `fake.rs` 编译为本地 `fake whisper.exe`，核验完整 WAV 字节后写固定中文 JSON。
  该程序没有真实模型、网络请求、下载或上传代码，不用于评估识别质量。
- fake 模型文件保存期望 WAV 路径及输出文本；调用记录用于验证缓存命中。
- 重复发布必须为 `Query failed`；错误后同一 MCP 仍能处理另一条语音。
- 单独检查内部 prepared-audio 超过外部 1 KiB 帧时仍可工作，以及最终文本超限。
- 未配置输出或显式后端、云端缺少上传许可时，不启动账号 daemon、不产生输出。
  使用无效账号配置与独占凭据句柄；这些断言不等于对所有文件打开尝试进行审计。

`tests/mcp_runtime.rs` 另复用音频 oracle 和 fake 后端校验 17 工具真实 IPC golden；
`tests/fixtures/mcp-protocol/routes.json` 锁定公开参数到 IPC 的精确映射。

```powershell
cargo test --offline --test mcp_voice_runtime --test mcp_runtime --test mcp_readonly_runtime --test mcp_image_runtime -- --nocapture
```

是否通过以本次实际进程退出码为准。编译成功、准备数据或旧基线均不代表宿主语音端到端通过。

## 响应与边界

长 JSON-RPC ID 即使能容纳在请求帧中，完整成功响应仍可能超限。此时应在 WAV 提交前返回安全错误；同一会话之后仍可接受合法短 ID 请求。相同音频、模型和消息编号的不同账号不得跨账号复用缓存。

进程和账号管理复用相邻只读夹具，不另造数据库解密实现。测试证明特定拒绝路径、无输出及进程生命周期，不是系统级文件打开审计；内部准备结果大于外部帧的测试也不能代替所有大小上下界测试。

环境见[测试说明](../../README.md)。真实模型识别质量、GPU 及远程云服务不属于合成协议进程验证；云端授权和回环传输由独立安全测试覆盖。
