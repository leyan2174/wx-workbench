# 架构优化实施与验收记录

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

## 基线与范围

检查对象为仓库根目录，分支 `main`，基线 HEAD 为 `d4e22a0`，远端为 `leyan2174/wx-workbench`。本报告针对已有大量未提交修改之上的继续优化，不将这些修改全部归为本阶段成果。历史报告保持原貌。

Windows x64 MSVC 构建使用独占目录 `C:/CodexLocal/wx-workbench-target-20260916`。没有访问真实微信数据、执行取钥算法或调用外部转录服务，没有提交、推送或发布。

## 已实施

### 朋友圈解析与资源归属

`src/toolkit/sns/decode.rs` 和 `parse.rs` 原来仅转发已有微信适配器，增加了一层无业务职责的路径。现删除这两个文件，由 SNS 装配直接引用 `adapters/wechat/moments` 的解码与投影类型。相册和既有解析测试继续运行同一实现。

解码器原来通过 `include_str!` 反向引用 Toolkit 的 `html_entities.json`。资源已移动到解码器同目录，移动前后 SHA-256 一致。同步更新架构文档、迁移表和独立图片夹具说明。没有重写解析算法或改变作者过滤、时区、输出字段及发布顺序。

朋友圈导出编排仍在 Toolkit，不能将此项视为整个 SNS 工作流迁移完成。

### 数据库 worker 材料生命周期

上一阶段新增 `DatabaseKeys` 后，解密步骤通过 `into_map` 取出普通映射，导致材料离开自动清零包装。校验与解密本来只需要借用。现移除该转换，保留包装直到解密完成或错误返回，沿既有析构路径清零。新增合成材料的 Debug 脱敏及析构清零观察测试。

`Step::WechatDecrypt` 的 daemon 只读授权已完成 broker 级验证，随后 `runtime_isolation` 使用本 checkout 二进制提交真实持久解密任务，断言任务成功和合成库解密输出。该目标也验证 CLI/Web/MCP 共享任务、断连幂等重试、取消回收及重启后 interrupted 恢复，未使用真实微信账号。

## 检查证据

### 一键准备移除隐式取钥

`daemon/operations/toolkit_run_prepare.rs` 原来在 `Missing` 时直接调用扫描器并保存材料，但准备请求没有相应显式取钥授权字段。现删除生产扫描、`load_saved` 和 `save_keys` 路径；`prepare` 从 worker 的 daemon 快照取得 `DatabaseKeys`。PreparedDecrypt、PreparedEmoticons 和 `prepare=true && !dry_run` 的 ExportAll 只获得数据库读取权限。保留原进程前置检查、路径验证和下游解密/发布语义。

生产材料继续由自动清零包装持有；表情缓存接收所有权后复用自身清零。测试模块中的存储助手只用于人工材料准备。旧自动扫描成功测试被替换为缺钥/损坏拒绝且文件不变的测试，HMAC、别名/硬链接路径和旧文件保护回归保留。新增真实一键解密缺钥反例：返回明确初始化提示，不创建密钥库或输出，不改变源和配置。

这是明确的兼容移除：普通准备不能自动获取密钥，需要先使用已有显式授权入口初始化；没有删除任何用户旧数据。

本阶段 all-target 编译通过；准备单元 7 通过，真实解密 13 通过、表情 7 通过，均无失败/忽略。broker 测试 29 通过、1 个子进程入口忽略。命令沿用 `cargo test --offline --locked --target x86_64-pc-windows-msvc`，分别选择 `--bin wx toolkit_run_prepare`、`--test run_decrypt_runtime --test emoticons_runtime` 和 `--bin wx worker_keys`，测试参数 `-- --test-threads=1`。日志前缀为 `C:/CodexLocal/wx-workbench-prepared-read-`，后缀为 `check.log`、`unit.log`、`runtime.log`、`broker.log`。

随后 `--test runtime_isolation` 通过 18 项，2 项 FFmpeg 条件测试保持忽略；严格 all-target Clippy 与格式检查通过。对应日志后缀为 `integration.log`、`clippy.log`、`format.log`。本次没有重跑此前完整根工程所有未受影响目标，最终验收仍须对应最终源码状态。

### 表情导出读取接入快照

`ToolkitOperation::ExportEmoticons` 已接入相同的 `READ_DATABASES` 授权，直接入口不再调用磁盘 `load_saved`。路径校验继续在业务执行前完成，材料包装保留至交给现有 `DbCache`；缓存沿用自身的析构清零。直接入口和一键准备仍共用同一个表情导出函数，没有新增缓存服务或第二套导出实现。一键准备自身的读取和缺钥扫描流程尚未迁移。

新增合成真实 CLI 用例，验证热 daemon 在磁盘密钥库损坏后仍可预览相同目录，重启后明确拒绝，源库、配置及旧材料文件保持不变，错误不包含图片材料且不创建导出目录。

首次运行 `emoticons_runtime` 为 6 通过、1 失败，日志 `wx-workbench-emoticon-read-runtime.log`：旧缺钥用例删除磁盘文件后假定热快照立即失效。按正式的显式重载/重启语义，在删除后关闭 daemon，再保留原缺钥失败断言。复跑 `cargo test --offline --locked --target x86_64-pc-windows-msvc --test emoticons_runtime -- --test-threads=1` 为 7 通过、0 失败/忽略，日志 `wx-workbench-emoticon-read-runtime-2.log`。未放宽路径、部分失败或源文件保护断言。

本切片 all-target check、严格 all-target Clippy 和格式检查通过，日志前缀 `wx-workbench-emoticon-read-`，后缀分别为 `check.log`、`clippy.log`、`format.log`。下载验证仅使用 loopback 合成服务；未访问真实媒体。此前全量和夹具矩阵保留原检查范围，此次未复跑未受影响目标。

### 前台离线解密接入快照

后续切片将 `Operation::Toolkit(Decrypt)` 接入已有 `READ_DATABASES` 授权，与持久任务 `Step::WechatDecrypt` 共用 daemon broker 和私有 stdin 材料交付。`daemon/operations/toolkit.rs` 不再为离线解密调用磁盘 `load_saved`，仍先执行原数据库路径校验，再交给原严格解密流程；材料包装在调用结束后清零。普通状态命令不获取此权限。表情导出及一键准备的磁盘读取仍待迁移。

`run_decrypt_runtime` 新增真实进程反例：先预览加载快照，损坏人工密钥库后仍能用热 daemon 快照解密并读到合成 SQLite 标记；显式关闭 daemon 后再次执行必须失败，并保留源、已发布输出和损坏存储。该行为对应已有显式失效/重启加载政策，不保证自动检测外部文件替换。

本切片执行 `cargo test --offline --locked --target x86_64-pc-windows-msvc --test run_decrypt_runtime -- --test-threads=1`：12 通过、0 失败/忽略（`wx-workbench-offline-read-runtime-2.log`）；同样参数的 `--bin wx worker_keys`：29 通过、0 失败、1 子进程入口忽略（`wx-workbench-offline-read-broker.log`）。随后 all-target check、严格 all-target Clippy 及格式检查通过，日志前缀 `wx-workbench-offline-read-`，分别为 `check-2.log`、`clippy.log`、`format.log`。这次未重复未受影响的全量和独立夹具矩阵，不能把前一全量记录替换为本切片最终全量通过。

### 监控失败恢复

`src/application/monitor/mod.rs`（当时位于 `src/toolkit/monitor.rs`）提取局部的输出后提交规则，并由生产循环调用。新增五项合成回归覆盖输出失败保留基线、重复不完整批次后恢复、空基线初始化、触顶/显式截断，以及畸形或倒退游标拒绝。复用生产批次解析和游标合并；未更改轮询等待或取消流程。测试证明的是批次处理契约，不替代真实管道断连和控制台取消测试。

完整日志位于 `C:/CodexLocal/`，不将编译检查计为运行测试。

| 检查 | 实际结果 | 日志 |
| --- | --- | --- |
| SNS 转发及资源迁移后的 all-target cargo check | 通过 | `wx-workbench-sns-boundary-check.log` |
| `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx toolkit::sns -- --test-threads=1` | 97 通过，0 失败，2 忽略 | `wx-workbench-sns-boundary-tests.log` |
| `cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/sns-download/Cargo.toml -- --test-threads=1` | 102 通过，0 失败，1 忽略 | `wx-workbench-sns-boundary-fixture.log` |
| `cargo fmt --all -- --check` | 首次发现初始化文件遗留排版差异，格式化该文件后通过 | `wx-workbench-boundary-format.log`、`wx-workbench-boundary-format-2.log` |
| `cargo check --offline --locked --target x86_64-pc-windows-msvc --all-targets` | 通过 | `wx-workbench-boundary-check.log` |
| `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings` | 通过 | `wx-workbench-boundary-clippy.log` |
| `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx cursor_recovery_tests -- --test-threads=1` | 5 通过，0 失败/忽略 | `wx-workbench-boundary-monitor.log` |
| `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx worker_keys -- --test-threads=1` | 29 通过，0 失败，1 子进程入口忽略 | `wx-workbench-boundary-keys.log` |
| `cargo test --offline --locked --target x86_64-pc-windows-msvc --test runtime_isolation -- --test-threads=1` | 18 通过，0 失败，2 项 FFmpeg 条件测试忽略 | `wx-workbench-boundary-runtime.log` |

SNS 忽略项分别为需要 Windows 符号链接权限的检查和父测试专用代理子进程入口；独立下载夹具仅忽略该子进程入口。未扩大忽略范围。

根工程全量和独立夹具矩阵的本阶段结果见下文；后续生产修改仍须按影响范围重新验证。

## 复杂度与保留项

### 根工程全量及清理竞态

本阶段执行 `cargo test --offline --locked --target x86_64-pc-windows-msvc --all-targets --no-fail-fast -- --test-threads=1`，33 个目标合计 2958 次通过、1 次失败、24 次忽略，退出码 101。完整日志 `wx-workbench-boundary-full-test.log`；计数包含独立测试宿主重复编译的共享模块，不等于互不重复的业务用例数。

唯一失败在 `native_migration_security::voice_rejects_parent_traversal_before_any_directory_creation` 的清理阶段：通用 Job 捕获器在一次性 CLI 退出后回收其 daemon 后代，而夹具随后又发送关闭 RPC，触发管道 EOF 竞态。现改为已有 `tests/support/cli_output.rs` 的有界 CLI 捕获器，由 `BootstrapCleanup` 单独验证和关闭 daemon。保留四项原始安全测试及路径、账号、输出保护断言。取消该夹具对通用 managed 模块的重复包含，其九项进程测试仍在主程序单元集执行，没有删除生产进程回收测试或扩大 ignore。

修复后 all-target 编译通过；`cargo test --offline --locked --target x86_64-pc-windows-msvc --test native_migration_security -- --test-threads=1` 为 4 通过、0 失败/忽略，日志 `wx-workbench-cleanup-fix-test.log`。未将首次全量失败改写为通过，也未在仅修改此夹具后重复运行所有未变目标。

随后格式复查与严格 all-target Clippy 再次通过，日志分别为 `wx-workbench-cleanup-fix-format.log` 和 `wx-workbench-cleanup-fix-clippy.log`。

`scripts/check-fixtures.ps1 -Offline` 已执行，27 个独立目标均编译通过；日志 `wx-workbench-boundary-fixtures.log` 与专属 target 下 `quality-fixtures/summary.json`。这项是编译矩阵，运行测试结果另列上表。

本阶段重新运行 `cargo clippy --offline --locked --target x86_64-pc-windows-msvc --bin wx -- -W clippy::cognitive_complexity`，退出 0、报告 4 项告警，日志 `wx-workbench-boundary-complexity.log`。请求分发 50 → 50、消息监控 30 → 26、延迟监测 31 → 31、朋友圈发布 26 → 26，阈值为 25。它们属于额外认知复杂度规则，不等于默认 Clippy 失败。监控降低来自明确的输出后提交责任；其余保留原路由、状态转移和发布顺序，不为数值机械拆分。

其他 worker 的密钥读取、首次账号初始化、Toolkit 工作流归位及最终集成验收尚未完成。保留有真实用途的格式投影和工作流；只凭名称包含 legacy 不足以删除功能。完整兼容移除清单见 `current-contract-cleanup-2026-09-16.md`，用户磁盘旧文件不自动删除。

本报告是持续实施记录，不是整仓完成声明。

## 当前源码后续优化

本节记录在上述阶段之后、以当前工作树为准的继续收口；前文的阶段性“尚未删除”描述保留为历史过程，不代表当前目录状态。

- `src/toolkit` 和 `vendor/wechat-decrypt` 已物理删除。正式 `wx toolkit` 仍是 CLI 命令分组，但不再对应源码架构层；第三方来源只保留署名、声明和 Git 历史，不保留可执行副本。
- 删除未使用且带 `dead_code` 豁免的 `scanner::scan_keys` 包装；显式进程名的受控扫描入口保持不变。
- `FilterLabel` 只表达业务筛选类型。CLI/MCP 字符串、大小写及兼容词汇的解析已移到 service 投影边界，业务模块不再解释协议标签。
- 延迟监控把 Sessions 回包结构校验、未来时间拒绝、数量上限和候选状态合并提取为纯函数。状态仍只在整批输出成功且来源完整后提交，复杂度从 31 降至 27。
- daemon 任务类型只接受 wire format 已有的 `snake_case`；删除 CLI `tasks status/show`、任务类型连字符形式、chat-plan 的 `--users`/`--write-plan-csv`、SNS 相册的 `--output-root` 及附件类型 `img`。第一方调用和文档已改用唯一正式拼写；没有保留隐藏转发层。

专项测试覆盖延迟状态合并、消息筛选投影、架构依赖、任务解析与 CLI 命令树；受参数调整影响的 `delta_plan_security`、`delta_runtime` 和 `sns_album_runtime` 分别为 7、8、12 项通过。

最终源码状态下，格式、all-target check 和 `cargo clippy --all-targets -- -D warnings` 均通过。额外认知复杂度审计退出 0，仍报告 `run_latency` 27、`run_monitor` 26、`dispatch` 49 三项阈值 25 的记录性告警，没有用 `allow` 隐藏。独立 fixture 编译矩阵 27/27 通过。首次根全量因 `runtime_isolation` 中把 `export-chats-native --users` 误改为另一命令的 `--user` 而有 1 个失败；恢复该正式参数后目标复测为 18 通过、2 个原有环境条件忽略。当前最终源码重新完整运行 33 个测试目标，合计 2923 通过、0 失败、20 忽略，退出 0。最终完整日志为 `C:/CodexLocal/wx-workbench-test-20260916.log`；all-target check、严格 Clippy 与复杂度日志使用同目录下的 `wx-workbench-check-20260916.log`、`wx-workbench-clippy-20260916.log`、`wx-workbench-complexity-20260916.log`，fixture 汇总位于 `C:/CodexLocal/wx-workbench-fixtures-target-20260916/quality-fixtures/summary.json`。

`KeyProvider::Auto` 已由明确的 `Saved` 替代：默认路径只使用统一 Store 中已验证的账号材料，缺失时拒绝，不再降级到内存扫描。已绑定 runtime 由现有进程绑定 broker seed 交付 32 字节账号材料，类型具备脱敏 Debug、长度验证和析构清零；`memory` 与 `account` 仍分别代表显式扫描和显式重启捕获。授权的 `account` 初始化通过同一 broker revision 原子提交账号材料与逐库材料，避免半写入。旧 CLI/wire 值 `auto` 被拒绝，首次初始化需明确选择可执行来源。

同一阶段移除 HTTP 查询 `username`、chat-plan 清单 `display_name`/`kind`、MCP 时间 `start_time`/`end_time`、搜索 `chat_name` 及消息类型 `emoji`/`voip`/`app`/`namecard` 兼容词。HTTP、MCP 和文件输入继续走原 service/daemon 调用链，只收紧协议投影；chat-plan 对未知字段启用拒绝，避免旧字段被静默忽略。对应专项结果为密钥存储 9、scanner 1、worker/broker 46（1 个父测试专用入口忽略）、CLI 2、Operation 5、初始化运行时 2、chat-plan 运行时 7、MCP 36、Web 6、chat-plan 单元 8 项通过，均无失败。

仍未完成的具体项：聊天目录仍有少量 `local_type` 媒体投影，消息证据和语音导出仍携带适配器内部定位坐标；这些需要按完整业务切片迁移，不能只改字段名。`.wx-cli`、`WX_CLI_*`、管道名和运行身份域分隔符参与当前账号及持久化身份，继续按 `project-naming.md` 保留，不当作可随意删除的命令别名。

## 后续通道核查

### DAT 恢复命名与秘密调试

DAT 产物仍是 JPEG/PNG 等编码文件字节，不是像素。内部类型改为 `RestoredImage`，统一入口和 V1/V2 实现使用 `restore`；提取、批处理、聊天目录、SNS 归档、材料验证、样本导出及相关测试同步。没有保留旧符号转发层，没有改变恢复算法、线上 `decoder` 字段或缓存目录。自动 Debug 曾可展开 `V2KeyMaterial` 的 AES 引用，现改为固定脱敏文本；这是消除暴露能力，不声称已发现实际日志泄露。

首轮编译发现 SNS 归档嵌套导入仍引用旧 dispatch，补齐后 all-target check 通过。日志 `wx-workbench-image-restoration-check.log`、`wx-workbench-image-restoration-check-2.log`。按 `--offline --locked --target x86_64-pc-windows-msvc --bin wx` 运行四组过滤测试：`attachment::decoder` 13 通过、`attachment::native_image` 28 通过、`toolkit::images` 13 通过、`extraction_uses_bound_image_material` 1 通过，均无忽略。日志前缀 `C:/CodexLocal/wx-workbench-restore-`，后缀对应过滤名（双冒号替换为连字符）。覆盖文件字节恢复、缺钥/坏格式、源与账号隔离、失败发布保护及调试脱敏，不只验证符号改名。

`scripts/check-fixtures.ps1 -Offline` 独立编译矩阵通过，日志 `wx-workbench-image-restoration-fixtures.log`；格式检查通过，严格 all-target Clippy 日志为 `wx-workbench-image-restoration-clippy.log`。此切片未重跑根全量和额外复杂度规则。SNS 运行时职责、媒体适配目录归位和后续材料通道仍未完成。

后续已修复下文定位的 Extract 缺口：`dispatch_state` 将同一 QueryLease 的数据库和图片材料传给提取函数；移除默认图片 provider、其全局配置构造器，以及失去最后调用方的 `config::load_config` 包装。显式参数 provider 保留，不改变获钥算法。提取改用有界 DAT 读取、固定源句柄及 ExportTarget 发布，保护账号数据库、密钥、缓存、配置和 msg 源树。缺材料不扫描；V2 缺钥诊断不再推荐已废弃的明文配置字段。此项没有改变密钥存储格式。

新增合成服务路由测试覆盖缺材料无输出、AES 恢复字节、覆盖与禁止覆盖、源/配置保护、硬链接拒绝、热快照和冷状态损坏拒绝、损坏 DAT 保留旧输出。该测试经过真实 dispatch_state、QueryState、DbCache、DPAPI 和发布实现，但不是独立 CLI 进程测试。首次图片算法回归为 11 通过 1 失败（缺钥诊断遗失原 AES key 文本）；恢复该识别文本并保留新的显式授权建议后 12 通过，未放宽测试断言。日志 `wx-workbench-extract-boundary-decoder.log` 和 `wx-workbench-extract-boundary-decoder-2.log`。

最后复核将 DAT 的二次路径打开改为 `Pin::read_bounded`，实际读取固定句柄并在读取前后及发布前验证源身份。新增精确上限、超限及上限溢出反例；不改变源文件。显式 provider 的参数已在构造时验证，因此其配置字段由不可能失败的 Result 简化为元组，删除不可达错误分支。

本切片验证（公共 cargo 选项为 `--offline --locked --target x86_64-pc-windows-msvc`）：

- 最终 all-target check 通过：`C:/CodexLocal/wx-workbench-extract-pinned-check.log`。
- `cargo test --bin wx pinned_read_enforces_size -- --test-threads=1` 与 `cargo test --bin wx extraction_uses_bound_image_material -- --test-threads=1` 各 1 通过，无忽略；日志分别为 `wx-workbench-extract-pinned_read_enforces_size.log`、`wx-workbench-extract-extraction_uses_bound_image_material.log`。
- `cargo test --test mcp_image_runtime --test native_migration_security --test stable_username_roundtrip -- --test-threads=1`：2 + 4 + 1 通过，无忽略；日志 `wx-workbench-extract-boundary-runtime.log`。这组在最终固定句柄读取调整前执行，读取调整另由上述服务路由测试验证。
- 严格 all-target Clippy 与格式检查最终通过；Clippy 日志 `wx-workbench-extract-pinned-clippy.log`。
- `scripts/check-fixtures.ps1 -Offline` 编译矩阵通过：`wx-workbench-extract-boundary-fixtures.log`。这是固定句柄辅助方法添加前的独立夹具编译结果，不是运行测试或最终源码的夹具矩阵重跑。

本切片没有再次运行根工程全量或额外复杂度规则。下列 2967 次全量通过是此次 Extract 修复前的集成基线，不代表新源码最终全量验收。历史资源定位回退未在本切片迁移为严格引用，不把授权/发布修复扩大表述成整个附件域已完成重构。

语音材料通道及音频概念审计完成后重新执行根工程全量：`cargo test --offline --locked --target x86_64-pc-windows-msvc --all-targets --no-fail-fast -- --test-threads=1`，退出码 0，33 个目标合计 2967 次通过、0 失败、23 次忽略。其中主程序 1413 通过、11 忽略。完整日志 `C:/CodexLocal/wx-workbench-consolidated-full-test.log`。计数包含共享模块在不同测试宿主中的重复运行，不等同于独立业务用例数；本轮未新增 ignore，也未以此替代独立 fixture 编译矩阵。测试期间没有修改生产源码。

`application/transcription/batch.rs::BatchTranscriber::transcribe` 仅在真正处理未完成语音时调用 `prepare_snapshot`。该快照仍直接读取正式 Store；不能仅给所有 ASR 操作添加启动时 READ_DATABASES，就声称完成迁移，否则没有语音或已完成转录的输入也可能提前因数据库材料错误而失败。待共享 worker 通道支持相应延迟读取语义后，再同步前台批处理、导出编排及任务步骤。

另一项已定位的容量差异：Store 的 MAX_KEYS 为 4096，而 operation worker 初始载荷沿用 MAX_REQUEST_BYTES（64 KiB），READ_DATABASES 当前将整份材料放入初始载荷。较大清单存在超限风险；本次没有放宽通信限额，也尚未实现分页/按需材料交付及大清单回归。此项属于未修复限制，不能将小型合成库成功外推为全部容量均已贯通。

后续只读核查收窄了修复方案（尚未实现）：现有 8 MiB 服务回包预算可容纳最多 4096 项、每名称最多 1024 字节的合法数据库材料，无需另建分页服务。纯数据库读注册应只授能力，首次实际使用时经已有管道鉴权读取；材料必须直接走类型化秘密回包，不能先转普通 `serde_json::Value`，且要覆盖解析失败/取消清零、scope/真实进程/撤销校验。任务 `refresh_redactor` 的前置快照读取也需审查，避免旁路破坏惰性。此方案不等于已解决大批量写钥请求容量，不允许借修复扩大初始化或图片权限。

全面媒体大模型方案未启动；后续依据独立只读审计做有限职责与命名整改，保留图片/SNS/表情实际密码能力。语音概念审计不构成其他媒体完整持久化与传递链路已审完的证明。

逐入口追踪曾发现 Extract 授权与发布缺口（现已修复）：`query::q_extract` 原先使用 `image_key::default_provider` 隐式提取内存材料，并直接写出文件。现有实现消费账号绑定 QueryLease 的图片材料和 RuntimeContext，使用受保护的共享发布及固定源文件句柄读取；旧默认 provider 已删除。合成回归覆盖缺失材料、覆盖策略、受保护路径、硬链接、热快照与冷启动失败。此前全量绿色不能作为这项修复的证据，应以本报告 Extract 专项记录为准；没有执行真实扫描。

当前 `parse_attachment_kinds` 明确拒绝 voice/video/file，现行 attachments/extract 不能被描述为已支持所有普通附件。已追踪的本地表情适配器通过摘要与来源校验后识别并返回原字节，不做 codec 转换。聊天视频和普通附件的其他入口仍须继续追踪。

新增[音频格式与密码材料审计](audio-key-boundary-audit-2026-09-16.md)：当前没有语音级密码材料或混入秘密存储的 codec 参数。修正两个内部索引命名，补秘密记录和 MCP 未知参数拒绝、ASR 恢复身份反例；保留图片密码材料和数据库解密。未修改存储版本或新增迁移。专项结果及未覆盖范围以该文档为准。

## 语音导出材料边界

### 后续 SNS 密钥流命名与验证

本节记录命名阶段，随后已完成职责迁移：实现和测试移入 `adapters/wechat/media/sns_keystream.rs`、`sns_keystream_tests.rs`，工作流不再注册或转发该模块。迁移后的根工程全量为 33 个目标、2974 通过、0 失败、23 项原有忽略；独立报告与完整验证状态见 [SNS 媒体适配器迁移](sns-media-adapter-2026-09-16.md)。以下“目录迁移未完成”仅描述之前命名阶段，不是当前目录状态。

共享 WxIsaac64 WASM 宿主已由 `video_runtime::VideoRuntime` 改为 `keystream::SnsKeystream`，视频前缀恢复方法改为 `restore_video`。图片、视频生产调用方和独立夹具同步更新，没有保留旧 Rust 转发入口；第三方资产与来源标识不变。它是图片和视频共用的密钥流能力，不是视频 codec 或 AES 实现。视频仍仅恢复前 128 KiB，尾部原样保留；`ftyp` 仅是格式检查，不是完整性认证。本轮未将模块迁入微信适配目录，不能把准确命名等同于目录职责迁移完成。

本轮 all-target cargo check 通过；`cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx toolkit::sns -- --test-threads=1` 为 97 通过、2 项原有忽略；同目标 `--test asr_video_security --test sns_album_runtime` 分别为 366 通过/3 项原有忽略和 12 通过/无忽略。均无失败；运行日志为 `C:/CodexLocal/wx-workbench-sns-keystream-tests.log`、`C:/CodexLocal/wx-workbench-sns-keystream-runtime.log`。这些目标包含共享模块测试，数字不是互不重复的业务场景数；没有访问私人媒体或外部转录服务。严格 Clippy、格式与独立夹具矩阵结果另行记录，旧全量结果不代表本轮最新源码已全量验收。

### 语音导出实现与专项验证

SNS 后续检查已完成：`cargo clippy --offline --locked --target x86_64-pc-windows-msvc --all-targets -- -D warnings`、`cargo fmt --all -- --check` 和 `scripts/check-fixtures.ps1 -Offline` 均退出 0。日志分别为 `C:/CodexLocal/wx-workbench-sns-keystream-clippy.log`、`C:/CodexLocal/wx-workbench-sns-keystream-fmt.log`、`C:/CodexLocal/wx-workbench-sns-keystream-fixtures.log`；逐夹具结果位于 checkout 专属构建目录的 `quality-fixtures/summary.json`。夹具脚本执行的是编译检查，不是运行测试。本轮没有重跑额外认知复杂度规则；此前四个热点的保留说明仍有效，不能将默认严格 Clippy 无告警表述为额外复杂度指标归零。

`src/daemon/operations/voices.rs` 原先直接加载磁盘 Store，绕过 daemon 已绑定的查询材料快照。现在 `Operation::Voices` 通过现有 broker 获得 `READ_DATABASES` 权限，由私有 worker stdin 交付数据库材料，并交给既有 DbCache 接管清零。缺失材料在创建输出目录前拒绝，不增加新密钥链路，不改变目录适配、筛选、原始音频和证据发布。

`tests/voice_runtime.rs` 新增真实 CLI/daemon 回归：合成 SQLCipher 目录、人工 DPAPI 材料；热快照在磁盘存储损坏后可继续读取目录，显式停止 daemon 后再次执行拒绝且不创建输出目录，源数据库和配置保持不变。筛选区间没有音频条目，故本用例证明材料通道和生命周期，不证明音频解码或转写。原四项查询分页、缺字段、失败和账户隔离测试保留。

首次新增测试因手工查询 daemon 与 CLI 操作启动机制混用失败，改用已有 RuntimeCleanup 和 CLI 自启动；第二次发现查询夹具缺少导出需要的 svr_id/data_index，仅在新用例补齐合成字段，没有放宽生产解析或原测试。第三次 `cargo test --offline --locked --target x86_64-pc-windows-msvc --test voice_runtime -- --test-threads=1` 为 5 通过、0 失败/忽略，日志 `C:/CodexLocal/wx-workbench-voice-read-runtime-3.log`；前两次失败保留在同前缀 `runtime.log` 和 `runtime-2.log`。

本切片 all-target cargo check、严格 all-target Clippy 和 `cargo fmt --all -- --check` 通过。前两项日志为 `C:/CodexLocal/wx-workbench-voice-read-check.log`、`C:/CodexLocal/wx-workbench-voice-read-clippy.log`。未重新运行根工程全量或额外复杂度规则，不能将本切片专项通过称为最终全量验收。ASR、其余媒体读取、初始化和职责整理仍待推进。

## 输出树基础设施迁移

### 问题与触发条件

`src/toolkit/directory_publish` 同时被聊天目录与 SNS 导出使用，实现只处理 Windows 文件身份、路径保护、来源绑定、预检和逐文件提交，不包含 Toolkit 命令、微信 schema 或业务查询。放在 Toolkit 使通用基础设施看起来由导出工作流拥有，也阻碍最终删除 `src/toolkit`。

### 已修改

- 实现与 24 项内嵌测试原样迁至 `src/infrastructure/output_tree`，新增窄的 `infrastructure` 装配模块。
- 聊天目录、SNS 导出/归档/相册及 daemon 入口直接依赖新模块；删除 Toolkit 声明，不保留转发层。
- 独立 `sns-publish` fixture 改为直接编译新路径；当前架构文档和迁移表同步更新。
- `tests/entry_architecture.rs` 新增语法树级反向依赖约束，拒绝基础设施引用 adapters、business、CLI、daemon、MCP、service 或 Toolkit，并确认旧目录不存在。

迁移没有改变 `Binding`、`ExistingPolicy`、锁文件名、绑定 manifest、候选预检次序、逐文件提交或部分失败报告。没有新增公开接口、磁盘迁移或用户操作要求。

### 验证与复杂度

- all-target cargo check 通过：`C:/CodexLocal/wx-workbench-output-tree-check.log`。
- 严格 all-target Clippy 通过：`C:/CodexLocal/wx-workbench-output-tree-clippy.log`；格式检查通过。
- 根模块输出树测试 24 通过、0 失败/忽略：`C:/CodexLocal/wx-workbench-output-tree-tests.log`。
- 独立 `sns-publish` fixture 运行测试 33 通过、0 失败/忽略：`C:/CodexLocal/wx-workbench-output-tree-fixture.log`。
- 架构测试 8 通过、0 失败/忽略：`C:/CodexLocal/wx-workbench-output-tree-architecture.log`。

本切片只移动责任归属，不拆分或重写控制流，因此模块函数的认知复杂度前后不变。没有重跑额外 `cognitive_complexity` 规则；该模块不在已记录的四个热点内，不将默认 Clippy 通过表述为复杂度告警已清零。

### 仍未完成

发布基础设施、配置事务、发布上下文和监控已经迁出 Toolkit。ASR、音频、聊天/SNS 导出、图片批处理、清理和运行状态仍有生产调用方，因此 `src/toolkit` 尚不能整体删除。后续必须以同样的调用方切换、旧实现删除和行为回归为单位继续迁移，不能仅为删目录复制代码。

## 公开源码候选收口

### 已修复

- 发布工作流不再监听标签推送。构建候选只能由 `workflow_dispatch` 启动；GitHub Release 和 npm 分别需要显式布尔输入、`v*` 标签以及独立受保护 environment。全局仓库权限为只读，仅 GitHub Release 任务申请 `contents: write`。
- Windows 二进制和两个 npm 包的候选均携带根许可证与第三方声明。工作流在发布前执行 npm 清单 dry-run；分发契约测试约束正式二进制名称、手动门禁和许可文件，避免标签或旧包名恢复成隐式发布入口。
- 缺少明确再分发依据的 SNS WASM 不再由默认构建嵌入、下载或自动发现。公开构建中的 `SnsKeystream::bundled` 返回脱敏的 `AssetUnavailable`；加密单视频只接受 `--wasm` 显式提供且固定哈希匹配的授权本地文件，明文 MP4 直通。
- 本地审计副本加入精确 Git 忽略和 Cargo 打包排除。它存在于工作目录时，`cargo package --list --allow-dirty --offline` 也不会把该文件收入包。内部 `sns-wasm-test-asset` 特性仅验证合法本地副本，不授予分发权。
- 当前候选中的三处个人绝对路径已改为仓库无关描述。
- `docs/diagrams` 中 38 个 PNG 没有图源、没有当前文档引用，且目录说明明确标注其未经当前源码核验。代码候选已删除这些二进制和历史副本，只保留重建规范；独立文档候选应依据当前源码生成有来源、可复核的新图件。

### 实际验证

- Node 分发契约：10 通过、0 失败。
- 将本地 WASM 物理移出仓库期间，`cargo check --offline --locked --target x86_64-pc-windows-msvc --all-targets` 通过；默认缺失资产反例 1 通过。检查结束后审计副本由同一脚本的 `finally` 恢复。
- 默认严格 all-target Clippy 通过；启用内部特性后的严格 all-target Clippy 也通过，未用全局 allow 隐藏条件编译告警。
- 内部固定资产密钥流测试 9 通过，覆盖合成向量、资源预算、隔离、视频前缀和资产错误。默认与内部两种构建模式的格式检查均通过。
- 从当前非忽略文件生成了无 `.git`、无 WASM、无数据库和密钥文件的临时源码快照。该快照使用独立 target 目录完成 Node 分发测试、格式、默认 all-target check 和严格 Clippy，均通过；证明默认源码候选不依赖私有历史或本地审计资产。
- 完整日志分别位于 `C:/CodexLocal/wx-workbench-public-no-wasm-*`、`wx-workbench-public-default-clippy.log` 和 `wx-workbench-wasm-dual-mode-validation.log`。日志位置是本机验证记录，不属于发布文件要求。

### 未验证与阻塞

- 干净快照首次根工程全量运行了 33 个目标，合计 2948 通过、8 失败、23 忽略，退出码 101。4 个失败来自外置 target 缺少项目要求的 `.checkout-owner`，3 个来自已删除 legacy 密钥机制的旧错误文本断言，1 个来自公开默认构建仍期待无许可 WASM 成功解密。没有把这次运行写成通过。
- 修复只涉及测试契约：外置 target 写入匹配候选路径的 ownership marker；legacy 用例改为验证正式 daemon 快照拒绝且源、旧材料和输出均不变；加密相册成功用例限定到内部特性，默认模式新增引擎不可用且不发布媒体的集成反例。三个失败目标随后分别为 12、13、12 通过，合计保留 1 个原有子进程夹具忽略；该定向结果没有被当成全量绿色。
- fixture 矩阵首次被错误地从 Windows PowerShell 5 调用，Cargo 正常 stderr 进度被宿主升级为异常，因此该次退出不算编译结论。改用脚本要求的 PowerShell 7 后，27 个目标真实运行；首轮定位出 MCP transport fixture 未同步惰性数据库读取协议，以及两个直接包含 SNS 适配器的 fixture 未声明内部特性名称。
- MCP fixture build 现在从生产 `service/worker_keys.rs` AST 只提取数据库回复协议所需类型、常量和编解码实现，不复制手写协议，也不扩大生产可见性；协议切片保留 `WorkerDatabaseKeys`。SNS fixture 只声明空特性供 `check-cfg` 识别，默认不启用、不携带资产。最终 27/27 fixture 编译通过，全部最新日志无 `warning:`。
- 相关独立运行测试通过：MCP CLI fixture 为 24 + 4 项，MCP voice host security 为 289 + 19 项（2 项原有环境权限/子进程忽略），SNS native 默认无资产为 1 项；均无失败。fixture 锁文件仅因新增测试期 `tempfile` 依赖按离线 workspace 更新。
- 完成上述修复后，当前工作树重新执行完整根工程命令：33 个目标合计 2956 通过、0 失败、23 项原有忽略，退出码 0；日志为 `C:/CodexLocal/wx-workbench-public-final-full-test.log`。这些数字包含共享模块在不同测试宿主中的重复运行，不等于 2956 个互不重复场景。后续若候选源码继续变化，冻结版本仍须重跑相同门禁。
- 相册入口没有外部 WASM 参数。公开默认构建中，需要 WxIsaac64 密钥流的远端加密图片或视频会报告引擎不可用；既有明文或无需该密钥流的路径不受此限制。首版说明必须保留此能力边界。
- 删除当前 vendor 快照不足以形成可直接公开的历史：固定 WASM 已由提交 `5644557` 引入，旧 vendor JS 也包含本机路径。现有仓库若直接改为 public 会公开这些历史对象。未获得授权前必须由发布负责人选择经审计的新历史/新仓方案；本轮没有擅自改写远端历史。
- 只读扫描未发现高置信私钥块、GitHub token、服务 token。三处 `secret-assignment` 命中已人工确认是验证字段过滤、授权优先级和缓存错误脱敏的合成常量；当前树中的 98 个 `wxid_*`、`gh_*` 与 `@chatroom` 唯一样例均为短、可读的测试命名，没有随机长账号标识。测试 SILK/PCM 可由仓内脚本从正弦波和静音重建并有逐文件哈希，SNS render PNG 是纯四色合成图；本地 WASM 已排除。扫描对象会随工作树变化，最终冻结后必须重扫。

这一收口不改变 WxIsaac64 算法、固定摘要、数据库字段或业务查询语义。它缩减的是无分发依据的默认资产能力和未经授权的自动发布能力，不代表 `src/toolkit` 已完成迁移，也不把发布准备状态表述为已经公开。
