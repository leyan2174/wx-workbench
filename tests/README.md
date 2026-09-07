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
cargo test --target x86_64-pc-windows-msvc --bin wx toolkit::asr::receipt::tests
cargo test --target x86_64-pc-windows-msvc --bin wx service:: -- --test-threads=1
cargo test --target x86_64-pc-windows-msvc --bin wx daemon::tasks:: -- --test-threads=1
cargo test --target x86_64-pc-windows-msvc --test runtime_isolation -- --test-threads=1
```

各 fixture 中未带 `--target` 的现行短命令以这里的 `CARGO_BUILD_TARGET` 为前置条件。历史命令保留其当时写法，不修改旧日志中的调用记录。

`runtime_isolation` 注册 `fixtures/daemon-tasks/runtime.rs`，实际启动隐藏的 daemon、任务工作进程和本地 Web，覆盖缺查询密钥、任务幂等、取消/停机、历史恢复与 CLI/Web 共享。仅使用临时合成数据；不依赖真实微信、FFmpeg、ASR 模型或企业账号。Job 后代回收测试中的 ignored 项由父测试显式启动，不应手动当作独立业务测试执行。

## 范围与证据

- 主仓测试包含单元、集成、真实子进程/命名管道和合成加密库；真实进程不等于使用真实微信账号。共享模块在多个套件中会重复执行，合计通过次数不是独立功能数。
- 默认 `ignored` 不计通过。FFmpeg/Frida 等可选测试须先核对本机依赖和测试行为，再定向执行；符号链接测试还可能受权限限制。不要无差别运行所有 ignored，部分条目是由父测试调用的子进程夹具入口。
- 部分差异 oracle 使用 Python 或 Node，属于构建/测试依赖，不能据此推断普通原生生产入口需要它们。不要为了文档检查重建 golden 或下载模型。
- 旧独立 harness 的编译器、依赖 rlib 与测试二进制必须匹配。已移除 manifest 的目录（如 `fixtures/mcp-contacts`、`fixtures/voice-cache-receipt`）使用其 README 指向的主仓入口，不恢复废弃 `target/` 或虚构 Cargo 命令。
- `fixtures/` 内文档中的“本次未运行”仅指对应独立夹具或历史取证流程，不能用它否定另行记录的主仓集中回归。最新结果统一见 [迁移验收记录](../docs/rust-migration.md)。

## 历史日志

fixture 的 README、REVIEW、HANDOFF、DESIGN 和原始日志保留各阶段的成功、失败及忽略原因，不把旧数字改成当前主仓数字。`run.ps1` 或 oracle 生成器可能覆盖固定名字的日志、源码副本或 golden；复核前阅读脚本，现行定向测试优先使用上面的 Cargo 入口，把新输出保存到独立位置。不要因测试日志过旧就删除引用它的证据。
