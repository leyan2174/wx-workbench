# 测试说明

本仓库仅支持 Windows x64 MSVC。从仓库根目录运行测试，使用已安装的 Rust、Visual Studio C++ 工具链与 libclang；构建依赖见 [账号提供器说明](../docs/account-key-provider.md)。不要使用真实账号或私人聊天作为默认测试输入。

## 统一入口

```powershell
# 仅影响当前 PowerShell 会话；使各 fixture 中省略 --target 的短命令也使用 MSVC。
$env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-msvc'
$env:PYTHONDONTWRITEBYTECODE = '1'
# 未自动发现 libclang 时，将 LIBCLANG_PATH 设为本机实际 DLL 所在目录。
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc
```

定向运行示例：

```powershell
cargo test --target x86_64-pc-windows-msvc --test mcp_readonly_runtime
cargo test --target x86_64-pc-windows-msvc --bin wx application::transcription::receipt::tests
cargo test --target x86_64-pc-windows-msvc --bin wx service:: -- --test-threads=1
cargo test --target x86_64-pc-windows-msvc --bin wx daemon::tasks:: -- --test-threads=1
cargo test --target x86_64-pc-windows-msvc --test runtime_isolation -- --test-threads=1
```

各 fixture 中未带 `--target` 的短命令以这里的 `CARGO_BUILD_TARGET` 为前置条件。

`runtime_isolation` 注册 `fixtures/daemon-tasks/runtime.rs`，实际启动隐藏的 daemon、任务工作进程和本地 Web，覆盖缺查询密钥、任务幂等、取消/停机、历史恢复与 CLI/Web 共享。仅使用临时合成数据；不依赖真实微信、FFmpeg、ASR 模型或企业账号。Job 后代回收测试中的 ignored 项由父测试显式启动，不应手动当作独立业务测试执行。

MCP 后台任务验收位于 `fixtures/daemon-tasks/mcp.rs`，同样由 `runtime_isolation` 注册：

```powershell
cargo test --target x86_64-pc-windows-msvc --test runtime_isolation mcp_tasks -- --nocapture
```

这三项测试启动真实 `wx mcp`、daemon、私有任务 worker 和 Web，解密合成 SQLCipher 联系人数据库并核验产物。覆盖提交/列表/详情/取消/事件、CLI/Web 互操作、幂等重试、响应丢弃、运行中 MCP 断连、任务取消及 worker 回收、身份核验后的 daemon 受控崩溃与 `interrupted` 恢复、配置/账号切换和宿主授权拒绝。响应丢失测试在 CLI 观察到 daemon 接受后丢弃 MCP 响应并终止该宿主，不是任意时刻的网络故障注入。

原查询和同步语音回归仍由 `mcp_runtime` 及独立 `fixtures/mcp-cli` 覆盖。独立夹具仅用于协议、宿主参数和认证查询适配，不能证明后台任务执行；`src/cli/mcp_tasks_tests.rs` 另外验证工具清单、参数、授权和取消/超时错误语义。真实账号、内存扫描、云上传以及真实模型质量不在本组验收范围。

## 范围与证据

- 主仓测试包含单元、集成、真实子进程/命名管道和合成加密库；真实进程不等于使用真实微信账号。共享模块在多个套件中会重复执行，合计通过次数不是独立功能数。
- 默认 `ignored` 不计通过。FFmpeg/Frida 等可选测试须先核对本机依赖和测试行为，再定向执行；符号链接测试还可能受权限限制。不要无差别运行所有 ignored，部分条目是由父测试调用的子进程夹具入口。
- 部分差异 oracle 使用 Python 或 Node，属于构建/测试依赖，不能据此推断普通原生生产入口需要它们。不要为了文档检查重建 golden 或下载模型。
- 独立 harness 的编译器、依赖 rlib 与测试二进制必须匹配。没有 manifest 的目录（如 `fixtures/mcp-contacts`、`fixtures/voice-cache-receipt`）使用其 README 指向的主仓入口，不自行拼装 Cargo 命令。
- 全面检查的范围、顺序和人工审核点见[测试计划](../docs/testing-plan.md)。构建和定向回归见[开发与回归验证](../docs/rust-migration.md)。单次通过不能代替真实模型质量、账号完整性和安装部署验收。

## 输出与隐私

测试输出保存到仓库外的私有目录，分别记录命令、退出码、通过、失败和跳过原因。公开文档描述测试范围与运行方法，不收录个人路径、原始响应或历次通过数量。

`run.ps1` 或 oracle 生成器可能覆盖固定名字的输出、源码副本或 golden；复跑前阅读脚本，定向测试优先使用上面的 Cargo 入口。真实账号、重启微信、提权、下载模型和云端上传需要单独确认；无人值守时跳过需要人工介入的项目，并记录原因。
