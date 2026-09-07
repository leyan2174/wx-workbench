# 真实 MCP 语音验收

> 本页各次运行数量与主线转报数量均为历史证据，不能相加或改写为本次文档同步的结果。生产入口已注册；下面命令用于以后复跑，新输出应另存，不覆盖原日志。本次未执行 Cargo。

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
$env:LIBCLANG_PATH = 'C:/CodexLocal/build-tools/libclang/clang/native'
cargo test --offline --test mcp_voice_runtime --test mcp_runtime --test mcp_readonly_runtime --test mcp_image_runtime -- --nocapture
```

是否通过以本次实际进程退出码为准。编译成功、准备数据或旧基线均不代表宿主语音端到端通过。

## 历史验收结果（2026-09-07）

先执行 `cargo build --offline --bin wx` 构建新宿主，再运行真实测试，未使用旧版二进制。

| 测试目标 | 实测结果 | 日志 |
| --- | --- | --- |
| `mcp_voice_runtime` | 6 通过，0 失败，0 忽略 | `runtime17-voice-svr-id.log` |
| `mcp_runtime` | 7 通过，0 失败，0 忽略 | `runtime17-regression.log` |
| `mcp_readonly_runtime` | 1 通过，0 失败，0 忽略 | `runtime17-regression.log` |
| `mcp_image_runtime` | 1 通过，0 失败，0 忽略 | `runtime17-regression.log` |

合计 15 项运行时测试通过，两个测试命令均退出 0。独立协议 harness 另有
33 个单元测试和 1 个进程测试通过，见 `protocol17.log`，不与上述数量混记。

首次语音运行暴露了本夹具将 `VoiceInfo.svr_id` 误写为 `server_id` 的问题。
只修正媒体表列名后重跑通过；消息表仍使用 `server_id`，未放宽失败分类、
字节比对、媒体 ID、工具数量或响应预算断言。

长 JSON-RPC ID 用例已验证：请求本身可容纳，但完整成功响应超限时，
返回 `Query result exceeds safe limit` 且没有 WAV 落盘；同一 MCP 随后使用
短 ID 成功导出。相同音频、相同模型及相同消息编号的不同账号，也验证了
缓存必须分别执行后端，不能跨账号复用。

main 随后报告全量 907 通过、0 失败、9 忽略、无 warning，最新 MSVC check
日志为 `wx-cli-voice17-verified-check.log`。这是 main 的全量验证结果，
并非本测试任务另行复跑的数量。

## 所有权与交接

本任务只修改四个上述运行时测试、`tests/fixtures/mcp-protocol/routes.json`
及本目录；没有修改生产代码。账号/进程管理复用只读夹具的 `support.rs`，
不另造 daemon 或数据库解密实现。本轮测试写入已完成，无运行中的测试进程。

Darwin 可只读复用 `accounts.rs`、`artifacts.rs`、`fake.rs` 和相邻
`mcp-readonly-runtime/support.rs`；独立安全审计写入其专属目录，避免共享夹具并发修改。

尚未由本任务单独证明的边界：

- 授权拒绝前是否尝试打开任何账号、模型或凭据文件。当前证明的是拒绝、
  无 daemon 启动和无输出副作用，不是系统级文件访问审计。
- 24 MiB 内部 IPC 与 16 MiB 外部帧的精确上下限临界值。当前实际验证的是
  内部准备结果大于外部 1 KiB 仍可用，以及完整最终文本/JSON-RPC 响应预算。
- 显式授权的云端上传流程和真实模型识别质量。本轮按要求没有启动网络或真实模型，
  前者留给隔离的安全审计，后者不属于本次端到端协议验收。
