# 真实导出问题：P1 修复

基线为 `445d64b`。本次只实现三个 P1，不修改外围导出脚本、密钥获取或保护机制。

## Windows 操作环境

`operation_client` 创建 Invocation 时，在 Windows 过滤名称以 `=` 开头的
驱动器伪环境变量（例如 cmd.exe 的 `=C:`）。普通变量、值中的 `=` 和空值保留。
非 Windows 的收集行为不变。服务端继续拒绝非法普通名称、NUL、大小写重复名、
保留变量及超出数量或字节限额的环境；直接提交伪变量仍被拒绝。

## 精确身份回传

当前账号的联系人、会话和消息目录证据用于确认稳定 username。
精确身份按原字符串匹配，优先于显示名匹配，不 trim、不改大小写、不按前后缀放行。
普通旧 username、`@openim`、`@qy_g`、`wxid_`、`@chatroom` 和系统账号使用同一证据规则。
消息目录证据必须关联到实际消息表；单独的 Name2Id 发送者记录不视为会话证明。
目录生成的 `unknown_<hash>` 是未映射表的占位引用，不是已证明的真实 username；
它仍仅由原有目录导出的专用路径处理，不能当作任意会话查询身份。

未命中精确身份时，history、attachments 等入口保留显示名精确/模糊匹配，
但候选不唯一时拒绝。**兼容性变化：**旧 attachments、export-chat 等宽松入口
曾在同名候选中排序取首项，现在统一拒绝歧义，不再静默选择另一个联系人。
调用方应保存并原样传回目录的 username，而非显示名。ResolveChat 和音频入口
仍保持原来的精确显示名契约，不新增模糊匹配。
前缀、昵称和其他账号的 username 本身不是账号存在证明。
物理目录与 schema 读取仍由微信适配器提供，协议入口不增加 SQL。

联系人与 session 精确证据检查后，若未配置消息分片（`msg_db_keys` 为空），
身份解析不探测消息目录，可继续按既有显示名规则解析联系人。
若配置了消息分片但目录不可读取，则未知名称返回源不可用错误，而不是“找不到”；
也不会回退到某个唯一显示名，以免把尚未核实的稳定 username 错配到另一个联系人。
已知显示名歧义仍可返回歧义拒绝。此规则不放宽导出能力探测的完整目录要求。

sessions 与基于 session 的导出清单新增 `exportable`。仅当已知折叠伪会话
`brandsessionholder` / `@placeholder_foldgroup` 在完整消息目录中没有对应表时，
返回 `exportable: false` 和 `skip_reason: "system_placeholder_without_message_table"`。
如果消息目录探测失败，列表仍正常返回；折叠项改为 `exportable: null` 和
`skip_reason: "message_source_unavailable"`，不能把能力未知误报为 true 或 false。
存在实际表的折叠项、普通无消息会话及其他系统账号不会被这一规则误判为不可导出。
`exportable: true` 仅表示当前会话能力证据，不保证有消息或附件，更不保证任何未来
导出成功；它不替代后续读取时的完整性检查、账号校验或资源可用性检查。

身份回归测试复现命令（全部为合成数据）：

```powershell
cargo test --bin wx chat_identity_tests
cargo test --test stable_username_roundtrip
cargo test --test mcp_audio_adapter
cargo test --manifest-path tests/fixtures/mcp-readonly-security/Cargo.toml
cargo test --manifest-path tests/fixtures/mcp-image-security/Cargo.toml
cargo test --manifest-path tests/fixtures/native-attachment-security/Cargo.toml
cargo test --manifest-path tests/fixtures/mcp-voice/Cargo.toml
cargo test --manifest-path tests/fixtures/mcp-voice-security/Cargo.toml
cargo test --manifest-path tests/fixtures/mcp-voice-host-security/Cargo.toml
```

`stable_username_roundtrip` 启动真实 daemon，使用独立临时 Account、合成加密数据库
和正式 DPAPI 存储夹具，经公开 CLI 将 sessions 返回值原样回传 history/attachments，
同时验证图片附件 ID、消息目录身份、同名拒绝、伪会话标记和跨账号隔离。
内部查询测试覆盖联系人/session/消息表三种证据、大小写与空白、精确身份优先、
无表发送者拒绝，以及严格消息定位；另覆盖 session 源正常但消息源缺失时，
sessions 与导出清单均可返回且折叠项能力为未知。安全 fixture 引用真实解析模块。

## 查询预算错误

普通查询的响应上限仍为 32 MiB，紧凑聊天导出仍为 256 MiB。
查询 v3 的身份握手、Oversize wire format 和请求限额不变。
history 的 JSON 查询失败时在 stderr 输出单个 JSON 对象，退出码为 1，stdout 不输出部分结果。
JSON history 复用静默启动选项，冷启动提示不会混入该错误流；文本模式保留启动提示：

```json
{"code":"response_limit_exceeded","operation":"history","response_limit_bytes":33554432}
```

文本模式明确显示该 code、上限和分页建议。例如：

```powershell
wx history '<sessions 返回的 username>' --limit 500 --offset 0 --json
wx history '<sessions 返回的 username>' --limit 500 --offset 500 --json
```

示例页大小不是保证：消息大小不同，仍可能需要更小的页。
不会猜测 `suggested_page_size`、自动增大预算或转用导出操作。
响应诊断不携带正文、username、数据库路径、密钥或原始错误链。

源码另有消息读取预算，例如候选集合最多 100,000 项。`history -n 200000`
可能先触发读取预算，而非 IPC 大小限制。对此 JSON 错误为：

```json
{"code":"query_read_limit_exceeded","operation":"history"}
```

不能将该错误伪装成 32 MiB 响应超限。请求帧超限、协议损坏、账号不匹配和
其他业务失败也不会被此修复统一改为响应超限。
无效参数使用 `InvalidData`，保留原有校验顺序和错误文字；零 limit、非法时间范围、
类型数量和分页整数溢出不会被包装成运行时读取预算耗尽。
现有 history 会读取 `offset + limit` 候选，且跨分片累计候选也受限；因此即便
页大小较小，大 offset 仍可能触发读取预算。此时需用 `--since` / `--until`
缩小时间窗口。本次没有修改排序、分页算法或时间端点语义，也不保证任意深度分页。

## 升级说明

升级时需让选定账号的 daemon 使用新二进制。查询 wire version 不变，但旧 daemon
不会获得新的身份解析或读取预算错误分类。本次验证只启动、停止测试夹具自己的
隔离 daemon，不操作用户的真实微信或真实账号服务。

## 未包含的工作

- P2 有界分块或流式聊天导出尚未实现，超过 256 MiB 的单响应仍会失败。
- P2 语音显示字段与导出 schema_version 尚未新增，现有输出保持兼容。
- P2 失效旧密钥的显式重新获取入口尚未实现；本次不放宽迁移验证或增加明文回退。
- P3 构建溯源版本命令尚未实现。

以上需要各自的发布、恢复、协议或构建测试，不能用本次 P1 的合成回归测试替代。
未使用真实账号进行大规模性能压测。折叠项的导出能力探测可能准备消息缓存，
其冷缓存成本没有以真实超大账号验证；测试只证明合成来源下的行为和安全边界。

## 验证记录

全部运行使用合成数据、人工密钥和隔离账号；没有读取真实微信数据或扫描微信进程。
本机实际使用 `C:/CodexLocal/build-tools/libclang/clang/native/libclang.dll`，
未移除 silk-codec/bindgen 或相关功能来绕过构建。

定向检查包括 operation 12 项、IPC outcome 6 项、身份及错误分类回归 27 项、
cleanup/history/username/voice 运行时回归 13 项、真实 cmd 和冷启动查询预算 3 项，
以及任务导出历史与 Web/CLI 共享任务各 1 项，均通过。
六个受影响的独立安全 fixture 共 524 项通过、4 项既有 ignored，均无编译告警。
fixture 为 mcp-readonly-security、mcp-image-security、native-attachment-security、
mcp-voice、mcp-voice-security、mcp-voice-host-security。

初次全量运行退出 101：4222 项通过、11 项失败、31 项 ignored。
已修正无消息源的联系人兼容、非法参数错误分类、合成任务身份凭据和清理时的进程退出竞态，
并通过对应定向回归。完整原始日志保留在
`C:/CodexLocal/wx-export-p1-all-targets.log`，不将该次运行记为通过。

第二次全量运行退出 101：4240 项通过、4 项失败、31 项 ignored。
运行期间另一 checkout 使用同一个 target 目录重建并替换了 `wx.exe`；
命令环境、查询预算和身份回传真实进程测试因此运行到了不含本次修复的二进制。
已与另一任务确认，双方改用各自独立 target，不改变测试断言。
该次日志为 `C:/CodexLocal/wx-export-p1-final-all-targets.log`，亦不计作通过。
本任务后续验证目录固定为 `C:/CodexLocal/wx-export-p1-target-20260916`。

隔离验收命令（完整输出分别保留为 `C:/CodexLocal/wx-export-p1-isolated-check.log`
和 `C:/CodexLocal/wx-export-p1-isolated-tests.log`）：

```powershell
$env:LIBCLANG_PATH='C:/CodexLocal/build-tools/libclang/clang/native'
$env:CARGO_BUILD_TARGET='x86_64-pc-windows-msvc'
$env:PYTHONDONTWRITEBYTECODE='1'
Remove-Item Env:CARGO_BIN_EXE_wx -ErrorAction SilentlyContinue
cargo check --offline --locked --all-targets --target x86_64-pc-windows-msvc --target-dir C:/CodexLocal/wx-export-p1-target-20260916
cargo test --offline --locked --all-targets --no-fail-fast --target-dir C:/CodexLocal/wx-export-p1-target-20260916 -- --test-threads=1
```

最终隔离验收：编译检查退出 0、零告警；全量测试退出 0，汇总 **4244 项通过、
0 项失败、31 项既有 ignored**，测试编译零告警。两个主程序分别为 1371 项通过、
10 项 ignored；真实 cmd 操作、冷启动下的两类查询预算、稳定 username 回传、
账号隔离、任务历史与 Web/CLI 互操作全部通过。
该次运行覆盖并通过前述失败场景，不依赖共享目录中的旧二进制。

本轮 Rust 文件的 `rustfmt --edition 2021 --config skip_children=true --check` 和
`git diff --check` 均通过。五个已修改的独立 fixture 文件格式整理后，
mcp-image-security、mcp-readonly-security、native-attachment-security 在隔离 target
再次测试，共 96 项通过、2 项既有 ignored、零告警。命令采用
`cargo test --offline --locked --manifest-path tests/fixtures/<fixture>/Cargo.toml --target-dir C:/CodexLocal/wx-export-p1-target-20260916 -- --test-threads=1`，
日志为 `C:/CodexLocal/wx-export-p1-isolated-<fixture>.log`。

另外对 `rg --files tests/fixtures -g Cargo.toml` 列出的全部 26 个独立 crate
执行了隔离目录下的 `cargo check --offline --locked --manifest-path <manifest>`，
全部退出 0、零告警。默认检查 `--all-targets`；依照原有禁用 libtest 的声明，
mcp-image-security 使用 `--lib --bins --test audit`，mcp-readonly-security 使用
`--lib --test security`，wav-publish 使用 `--lib --test publisher`。
mcp-history-compat 和 plan-selection 使用 `--features runtime --all-targets`，
且 `CARGO_BIN_EXE_wx` 固定为上述隔离目录的 Windows debug 二进制。
这些范围未通过修改 Cargo.toml 或新增 ignored 改变。
完整日志为 `C:/CodexLocal/wx-export-p1-matrix-<fixture>.log`。

既有 ignored 包括 FFmpeg/ffprobe 集成、需要 Windows 符号链接权限的用例、
显式 Frida 集成和由父测试启动的隐藏子进程 fixture；本次没有新增 ignored。
`--all-targets` 会重复执行 wx/wx-toolbox 和部分 fixture 中的同一测试，
因此汇总数量不是互不重复的业务场景数量。

## 修改文件

以下均为仓库相对路径；未改包版本、锁文件、外围 skills 或 vendored Python。

```text
docs/architecture.md
docs/export-p1-fixes.md
src/adapters/wechat/messages/mod.rs
src/adapters/wechat/messages/session_identity.rs
src/adapters/wechat/messages/sessions.rs
src/cli/history.rs
src/cli/mod.rs
src/daemon/query.rs
src/daemon/query/chat_identity.rs
src/daemon/query/chat_identity/tests.rs
src/daemon/query/decode.rs
src/daemon/query/export.rs
src/daemon/query/export_delta.rs
src/daemon/query/mcp_audio.rs
src/daemon/query/message_read.rs
src/daemon/query/message_read_tests.rs
src/daemon/query/strict_message.rs
src/daemon/server.rs
src/ipc.rs
src/ipc/OUTCOMES.md
src/ipc/outcome.rs
src/ipc/outcome_tests.rs
src/service/operation_client.rs
src/service/query_client.rs
tests/bootstrap_cleanup.rs
tests/fixtures/daemon-tasks/runtime.rs
tests/fixtures/delta-query/query_tests.rs
tests/fixtures/mcp-image-security/lib.rs
tests/fixtures/mcp-image-security/real_cache_query.rs
tests/fixtures/mcp-readonly-security/lib.rs
tests/fixtures/mcp-readonly-security/security.rs
tests/fixtures/mcp-refer/query_tests.rs
tests/fixtures/native-attachment-security/query_boundary.rs
tests/mcp_audio_adapter.rs
tests/operation_cmd_environment.rs
tests/query_response_limit.rs
tests/stable_username_roundtrip.rs
tests/support/bootstrap.rs
tests/support/image_media_adapters.rs
tests/support/mcp_failure.rs
tests/support/message_identity_adapters.rs
tests/support/message_read_adapters.rs
```
