# Fixture Check Matrix

可在根目录执行 `./scripts/check-fixtures.ps1`（离线时添加 `-Offline`）复现本表编译范围；脚本另检查 `src/crypto/test-harness`，保留逐项日志及 JSON 汇总。检查契约见[质量检查](../../docs/quality-checks.md)。编译检查不等于运行了全部独立测试。

本表定义独立夹具的有效检查范围，包括下列 26 项和两个 runtime feature。告警作用域见[夹具告警规则](WARNINGS.md)，执行与人工审核要求见[测试说明](../README.md)。根项目使用 `--all-targets`；检查结果以实际命令、退出码和日志为准。

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

- `mcp-image-security`、`mcp-readonly-security`、`wav-publish` 配置 `[lib] test=false`。它们通过具名 integration 调用普通库；强制 libtest 会分别引入生成文件的相对测试路径、真实 query/cache 单测依赖、应用级 ASR pipeline 单测依赖。不扩充模拟应用，也不删除根测试。无 bin 的夹具中 `--bins` 不增加目标，可省略。
- 两个 voice-host 夹具使用 `tests/support/managed_process.rs`，仍引用真实 `src/windows_process/managed.rs`，保留其单测。夹具不消费 `isolate_standard_handles`，不包含 Frida 管道测试；根测试覆盖完整 `windows_process.rs`。两者使用 `--all-targets`，不设置 `test=false`。
- `mcp-history-compat` 和 `plan-selection` 的 runtime integration 使用 `required-features=["runtime"]`；默认 all-targets 不启用它们。检查这两个目标时使用 `--features runtime --all-targets`，并设置 `CARGO_BIN_EXE_wx` 为当前 checkout 的根项目构建产物。执行需要真实 wx daemon/CLI 与合成账号夹具，编译通过本身不代表运行通过。

## 告警作用域

- 只在夹具的生产模块嵌入点按需使用 `allow(dead_code)`，逐处注明该夹具未消费的 API。公开业务模块的 `allow(unfulfilled_lint_expectations)` 仅适配 fixture 可达性变化，生产 expect 保持原样。不添加 crate 级 allow(warnings)。
- contacts/cache 的测试专用重导出和共享 message-read 别名仅在需要的模块/导入级处理 unused_imports；不保留未消费的兼容别名。
- voice-host-security 生成脚本在既有测试模块剥离之后，按语法树精确移除仅属这些测试的 BackendKind/unpack 导入，不修改生产源码或生成目录文件。
- 告警抑制不得更改业务断言、真实算法、授权或账号行为。测试使用合成账号；静态检查与实际编译结果分开记录，不用关闭整个程序的告警代替验证。
