# Fixture Check Matrix

可在根目录执行 `./scripts/check-fixtures.ps1`（离线时添加 `-Offline`）复现本表编译范围；脚本另检查 `src/crypto/test-harness`，保留逐项日志及 JSON 汇总。本轮修订见[代码质量记录](../../docs/code-quality-review-2026-09-16.md)。编译检查不等于运行了全部独立测试。

本表定义独立夹具的有效检查范围。集中复核已完成：根工程和下列 26 项均零错误、零告警，包含两个 runtime feature；命令与日志说明见[编译告警维护](../../docs/compiler-warnings.md)。根项目使用 `--all-targets`。

在仓库根运行 `cargo check --manifest-path tests/fixtures/<fixture>/Cargo.toml --target x86_64-pc-windows-msvc <targets>`。以下是完整 26 项范围，不把普通库单测跳过来隐藏告警。

| Fixture | targets |
| --- | --- |
| asr-cache-security | `--all-targets` |
| asr-local | `--all-targets` |
| asr-video-security | `--all-targets` |
| attachment-refs | `--all-targets` |
| contact-rows | `--all-targets` |
| emoticons-catalog | `--all-targets` |
| emoticons-download | `--all-targets` |
| mcp-audio | `--all-targets` |
| mcp-cli | `--all-targets` |
| mcp-history-compat | `--all-targets` |
| mcp-image-listing-parity | `--all-targets` |
| mcp-image-security | `--lib --bins --test audit` |
| mcp-protocol | `--all-targets` |
| mcp-readonly-security | `--lib --bins --test security` |
| mcp-voice | `--all-targets` |
| mcp-voice-host | `--all-targets` |
| mcp-voice-host-security | `--all-targets` |
| mcp-voice-security | `--all-targets` |
| native-attachment-security | `--all-targets` |
| native-image | `--all-targets` |
| plan-selection | `--all-targets` |
| sns-album-render | `--all-targets` |
| sns-download | `--all-targets` |
| sns-publish | `--all-targets` |
| sns-video-native | `--all-targets` |
| wav-publish | `--lib --bins --test publisher` |

## 声明范围与例外

- `mcp-image-security`、`mcp-readonly-security`、`wav-publish` 原有 `[lib] test=false` 保持不变。它们通过具名 integration 调用普通库；强制 libtest 会分别引入生成文件的相对测试路径、真实 query/cache 单测依赖、应用级 ASR pipeline 单测依赖。不扩充模拟应用，也不删除根测试。无 bin 的夹具中 `--bins` 不增加目标，可省略。
- 两个 voice-host 夹具改用 `tests/support/managed_process.rs`，仍引用真实 `src/windows_process/managed.rs`，保留其单测。没有 `isolate_standard_handles` 消费者，不再连带引入 Frida 管道测试；根测试继续覆盖完整 `windows_process.rs`。两者仍使用 `--all-targets`，没有新增 `test=false`。
- `mcp-history-compat` 和 `plan-selection` 的 runtime integration 原有 `required-features=["runtime"]` 不变；默认 all-targets 不启用它们。本轮额外用 `--features runtime --all-targets` 编译了这两个目标，设置 `CARGO_BIN_EXE_wx` 为根项目构建产物。执行需要真实 wx daemon/CLI 与合成账号夹具，编译通过本身不代表运行通过。

随后两个 runtime integration 均已实际通过。计划夹具新增 session 库密钥后补调用现有显式迁移助手，保持原断言；首次失败和定向复测均记录在编译告警维护文档中。

## 本轮告警接线

- 只在矩阵已报告的生产模块嵌入点使用 `allow(dead_code)`，逐处注明该夹具未消费的 API。公开业务模块的 `allow(unfulfilled_lint_expectations)` 仅适配 fixture 可达性变化，生产 expect 保持原样。不添加 crate 级 allow(warnings)。
- contacts/cache 的测试专用重导出和共享 message-read 别名仅在需要的模块/导入级处理 unused_imports；移除 mcp-cli 的旧 HostOutputGuard 别名和 native-attachment 的未消费 strict_message 别名。
- voice-host-security 生成脚本在既有测试模块剥离之后，按语法树精确移除仅属这些测试的 BackendKind/unpack 导入，不修改生产源码或生成目录文件。
- 不更改业务断言、真实算法、授权或账号行为；未访问真实账号。静态修复与实际编译结果分开记录，不用关闭整个程序的告警代替验证。
