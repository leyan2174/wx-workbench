# Rust migration and regression ledger

状态：个人微信与通用功能移植之后，已完成 daemon 任务服务的第一阶段归并；企业微信仅维持既有能力，实机与部署验证边界不变。

## Daemon 任务归并验收（2026-09-07）

已有 Web 的 12 类任务改由同一账号 daemon 持有队列、历史、日志、取消及 Windows Job；新增 `wx tasks`，Web 作为任务 RPC 适配。查询缓存懒初始化，密钥初始化失败可重试；认证任务管道校验版本、运行身份、进程身份及私有令牌。旧 Web 的五个调度/设置文件已删除，领域算法保留。详见 [任务服务](daemon-tasks.md) 与 [当前架构](architecture.md)。

- 全量回归：`C:/CodexLocal/wx-cli-daemon-service-suite-final.log`，20 套件合计 **1370 passed / 0 failed / 12 ignored**，退出 0、无编译警告。主程序 950/0/8，共享安全套件 209/0/2；增加的 ignored 是由父测试调用的隐藏进程树夹具，其父测试已通过。
- 全量之后进一步收紧 Web 停机行为：Web 不再自动拉起已被用户停止的 daemon。修改后 `wx-cli-daemon-service-web-final.log` **21/0/0**，`wx-cli-daemon-service-runtime-final.log` **15/0/2**。后者还新增个人微信合成加密库经 daemon 解密、聊天导出及正文核验；不得把这两个增量只归因于前面的全量日志。
- 进程验收覆盖同账号 CLI/Web 共享、Web 首次启动、关闭后任务可继续查询、明确停机不自动重启、设置冲突、同 ID 不重复导出、跨账号不可见、历史重启恢复、运行中查询、取消/停机回收工作进程；Job 后代回收另有真实隐藏子孙进程测试。
- `wx-cli-daemon-service-check-final.log` 记录 MSVC check；图形日志为 `wx-cli-daemon-service-diagrams.log`。当前 28 个 Mermaid 图块和 5 个别名已同步，历史图不改写。最新图块 02、03、04、16、19 反映服务归属变化。

第一轮失败日志 `wx-cli-daemon-service-suite-01.log` 保留：严格枚举字段、Windows 文件名终止符、连接建立前的管道忙竞争已修复并复验；既有 SNS 慢响应测试的接受连接显式恢复阻塞模式，生产下载逻辑未修改。

本次没有把全部 toolkit 直接入口或 MCP 17 个工具移入任务服务，`cli/task_worker.rs` 仍是类型化执行桥。Web 自动图片、企业离线查询及部分宿主编排保留原路径；前端资产未改写，旧前端 61/0 不作为本次新跑结果。所有新验收使用合成数据；真实账号、模型/GPU、云服务及已安装发布包未重验。以下章节均保留各自时点的结果，不覆盖本节后续改造。

## 此前文档同步复核（2026-09-07，历史）

README、Agent 使用说明、协议与领域文档、历史审计定位及静态架构图按当前源码同步，入口见 [文档索引](README.md)。现行图从架构正文生成；27 张现行结构/流程图与 1 张历史设计图分开标识，旧图原件保留在图目录的历史归档中。

文档同步期间重新执行 MSVC `cargo check` 和完整 `cargo test --target x86_64-pc-windows-msvc`，均退出 0；本轮仍为 **20 套件、1325 passed、0 failed、11 ignored**，编译保留 9 类未使用警告。18 项公开入口的实际二进制 `--help` 检查全部退出 0，不执行扫描、导出、识别或清理。完整本机日志为 `C:/CodexLocal/wx-cli-doc-sync-check.log`、`C:/CodexLocal/wx-cli-doc-sync-tests.log` 和 `C:/CodexLocal/wx-cli-doc-help-check.log`。

本次仅改文档、图件及其生成辅助脚本，未改 Rust 生产逻辑。前端 61/0 和额外 8 次可选测试仍引用下方上一轮日志，本次未重跑；真实账号、模型、云服务及安装发布包未验证。以下历史阶段的原始测试数量和失败记录保留，不替换为本轮数字。

## 九项目标结果与边界

当前权威证据是精简后 **20 套件 1325 passed / 0 failed / 11 ignored**，不是下方旧阶段的计数；个人前端 **61/0**。主线已核对日志、清理后的八项路径、关键图像与差异检查；以下是本轮交付结论及边界。

| 原目标 | 当前结果 | 验收边界 |
| --- | --- | --- |
| 1. 尽可能以 Rust 替代 Python/Node | 个人微信生产入口和业务编排已有原生 Rust 路径；可选 Python 模型桥、npm 包装明确保留 | Python Whisper/PyTorch 推理不是 Rust；浏览器 JS、Frida Hook、WASM 不冒称已删除 |
| 2. Rust 架构与职责优化 | CLI、IPC/daemon、领域解析、媒体及发布分层；ASR Job/drain 共用 windows_supervision，私有文件权限共用 private_file | 静态职责划分和自动化回归不等于所有实机性能指标已验证 |
| 3. 功能检验与回归 | 精简后全量 1325/0/11；前端 61/0；默认忽略中的 8 次可选执行另行通过 | 2 项父进程/子进程夹具及 1 项符号链接权限测试未显式运行；真实账号、模型/GPU、真实云服务不在合成验收内 |
| 4. 注释尽量中文 | 重构涉及的模块已有中文职责、安全边界和复杂逻辑说明 | 保留协议名、第三方内容及部分原注释，不宣称全仓逐行中文化 |
| 5. 详细架构图与功能移植 | architecture.md 按导航/主题保留详细调用、安全边界和时序图；个人微信能力已有映射及上述自动化证据 | 企微明确排除；最新图渲染报告由本轮图 QA 单独交付，模型/真实媒体/部署不能由图验证 |
| 6. 迁移后原有代码整理 | ASR、SNS 及主线 legacy/包装精简已落地并进入精简后全量回归 | 不为减少文件数删除仍需保留的明确兼容路径、参考源码或模型 |
| 7. 移除不必要文件目录 | 白名单核验清理 1910 文件、682189737 逻辑字节，源码/日志/模型/活动缓存保留 | 逻辑大小不等于实际释放磁盘空间；保留物不一概视为冗余 |
| 8. 最后一轮合并删除优化 | ASR 共享监督净减 50 行、SNS 4 包装净减 16 行、失效 legacy 链及主线 3 包装已移除 | 行数是主线记录，不证明最优或功能完整；证据是精简后的 1325/0/11，check 仍有 9 警告 |
| 9. 尽可能并行子任务 | ASR、SNS、Web/测试、文档及主线清理已由多条工作线并行交付 | 并行交付不替代主线最终逐目标核验 |

**非企微必需能力的实际未实现项：本轮源码可达性审计未确认额外缺失项。** 这不是从无 TODO 或 38 模块/544 符号数量推定全覆盖；真实历史完整性、任意媒体播放、模型/GPU/真实云端质量、已安装版本与部署环境仍是未验证边界。源码、自动化日志、关键图和差异检查已核对；当前使用入口见 [文档索引](README.md)。

## 完整目标验收范围

1. 尽可能将 Python、Node.js 生产代码迁移为 Rust，不以兼容调用完成迁移验收。
2. 重构与优化 Rust，使架构及模块职责清晰。
3. 全过程进行功能和回归验证，不遗失功能。
4. 重构涉及的代码注释尽量使用中文。
5. 提供详细架构图；wechat-decrypt 的个人微信和通用基础功能属于移植范围。按用户最新要求排除企业微信。
6. 功能移植后继续整理优化原有代码，补充有用的中文注释。
7. 全功能验收后删除不必要的代码、文件和目录，保持项目简洁。
8. 最后再次整体优化；可合并的功能模块合并，可删除的代码删除，以尽可能少的代码保持完整功能。
9. 尽可能多地开启子任务并行开发；当前执行环境最多同时运行六个，完成后复用补位。

本轮开发与自动化验收已完成。末期清理仅删除核验白名单内的可再生产物和确认无调用的代码；仍在使用的兼容实现、参考源码、测试证据与已安装稳定版本保留。

用户最新确认：wechat-decrypt 的个人微信和通用基础功能需要移植，包括未通过现有 toolkit 暴露的能力；企业微信不再作为目标验收条件。不继续移植扩展企微，保留既有代码，必要修复仅维持构建/回归。
静态起点清单见 [完整源码能力盘点](legacy-capability-inventory.json)：38 个生产模块、544 个 Python 类/函数、45 处 CLI 参数声明。
清单不是完成率报告，每项能力必须有迁移去向和验证，不能通过删除旧模块或隐藏入口缩小范围。

## 精简后当前验收快照（2026-09-07）

本节是当前状态，优先于后文所有历史阶段中的“当前”“待完成”“未编译”及旧计数；不据历史 TODO 再推定有未实现功能。企业微信不继续移植扩展，保留既有代码，必要修复仅维持构建/回归，不以企微功能完整性作为目标条件。本轮开发、精简及自动化验收收尾完成，不将实机或部署的未验证事项写成已通过。

- **精简后全量。** 主线会话 98170 已终态退出 0；`C:/CodexLocal/wx-cli-post-cleanup-full-regression.log` 的 20 组结果合计 **1325 passed、0 failed、11 ignored**，主程序 908/0/7、共享 harness 209/0/2。覆盖本次 ASR 共享监督及 Send、HTTP→IPC 两项新增合成测试、native rich、source 等集成修改。前次 **1321/0/11** 仅为精简前功能快照，不能用于证明本次清理效果。
- **检查与前端。** `C:/CodexLocal/wx-cli-cleanup-check.log` 的 MSVC check 退出 0，9 条警告仍保留，不称无警告；主线 diff check 为 0，仅 attachments/contacts 的两条 CRLF 提示，未因此改写这两文件。个人 Web 的 `web-ui-personal-final.log` 为 **61/0**，报告 `C:/CodexLocal/wx-cli-web-qa-personal-final/report.json`；自动图片与 rich 已接线并验证相应合成场景，不再列为待实现。
- **可选测试单列。** 默认全量仍如实记 11 ignored。`C:/CodexLocal/wx-cli-native-optional-tests.log` 的 4 项及 `C:/CodexLocal/wx-cli-audio-optional-integration.log` 的 2+2 项已显式通过，覆盖本地 FFmpeg/Frida 合成用例；不改写默认 ignored，也不把重复执行算成新的独立功能。余下 2 项父进程/子进程夹具及 1 项符号链接权限测试未显式运行。
- **源码精简。** ASR 的 `local.rs`、`local_python.rs` 共用 `windows_supervision` 的 Job/输出排空逻辑，主线记录净减 50 行；SNS 4 个包装净减 16 行；主线移除失效 legacy 调用链和 3 个包装。共享 `private_file` 保留，实际私有 DACL 的 protected 位未弱化。净行数来自主线清理记录，功能验证依据是上述精简后回归，不是行数。
- **产物清理。** `C:/CodexLocal/wx-cli-prune-result.log` 的核验白名单清理共 1910 个文件、682189737 逻辑字节（约 650.59 MiB）；逻辑大小不等于实际磁盘释放量。源码、日志、模型及根目录活动构建缓存/产物保留，未以删除依赖或参考实现代替迁移验收。
- **运行与实机边界。** 直接 exe 走原生入口，不需要 Node；Node 为可选 npm 包装。旧批量转录在执行识别时按账号配置选择引擎，`transcription_backend` 缺省 `local` 使用 Python/Whisper/PyTorch，模型名缺省 `base`，可能下载权重但不上传音频；MCP 的 Python 路径另需显式 `--configured-local-python`。不声称模型推理已纯 Rust。合成测试及前端 QA 不证明真实账号历史覆盖、任意媒体可播放、真实模型/GPU 识别质量或已安装发布包/部署环境验收。

## 精简前交付增量与验收边界（历史快照）

以下表格和阶段段落仅记录精简前交付，不再作为当前待办清单；其“尚未获得”“进行中”以及 1321 统计由上方精简后快照更新。真实环境和部署边界仍如实保留，不把未验证推断为未实现。

本节优先于后文历史阶段的入口、缺口及测试描述。企微不继续移植扩展，保留既有代码，必要修复仅维持构建/回归；企微功能完整性不计入当前目标验收。**当前功能里程碑已通过 Rust 全量回归 1321/0/11 ignored（20 套件）和个人 Web 前端 61/0；最终精简进行中，整个目标尚未完成。**

本次主线执行并核对日志：最新 MSVC check 通过；AES 修复后的 `cargo test --target x86_64-pc-windows-msvc` 已编译并运行通过，`C:/CodexLocal/wx-cli-personal-full-regression.log` 共 20 条成功结果，合计 **1321 passed、0 failed、11 ignored**。其中主程序 905/0/7，共享 harness 208/0/2；跨套件重复执行不作为独立用例或功能数量。此为最终精简开始前的功能快照，不覆盖随后改动。

`C:/CodexLocal/web-ui-personal-final.log` 为 **61 passed checks、0 failures**，报告 `C:/CodexLocal/wx-cli-web-qa-personal-final/report.json`，覆盖桌面/手机实际前端资产与合成 API 下的自动图片、rich、通知、身份绑定和失败回退。该前端验证与 Rust 全量测试分开计数；Dalton 新增的两项 HTTP→IPC 合成测试仍在进行，不宣称其已通过，也不宣称真实账号、模型、GPU 或私人媒体已验收。

历史快照：`wx-cli-personal-integration-test.log` 曾记录 874/5/7，不改写为成功；其四项保留企微回归已由 `wx-cli-retained-acl-ten-tests.log` 的 10/0 和 `wx-cli-retained-acl-exact.log` 的精确 ACL 1/0 闭环。Web 监控夹具去掉错误额外 data 包装、改用真实 `Response::ok` serde flatten 后，`web-monitor-focused-msvc.log` 为 1/0、885 filtered out，生产行为未改变。随后 history source、自动图片与 rich 已进入本次功能全量快照，不再沿用旧“未编译”状态。

后续阶段单列：Russell 正在精简 ASR 重复实现，Peirce 正在精简 SNS 包装，主线正在清理失效 legacy 调用链。尚未宣称这些精简完成；改动后的编译、全量回归、发布及最终删除审计需要新证据，不能沿用 1321/0/11。共享 `toolkit/private_file.rs` 已用于自动图片与保留企微密钥文件，在敏感内容首次写入前设置当前用户私有 DACL 和实际 protected 控制位；源码关系图见 `architecture.md`，不是放宽或跳过 ACL 断言。

| 个人微信能力 | 当前源码证据与已实现行为 | 尚未获得的验收证据 |
| --- | --- | --- |
| 历史消息表目录 | `src/cli/export_messages.rs` 发送 `ExportDirectoryCatalog`；`src/daemon/query/export_directory/catalog.rs` 枚举实际 `Msg_*` 表，不以 SessionTable 决定范围。联系人/Name2Id 补身份；无法映射时保留完整 `unknown_<hash>`、`identity_status`、表名与来源并可导出。catalog 核心 5 项、CLI 选择 4 项合成测试通过；本次 history source 接线进入新全量快照 | 真实历史覆盖及完整工作流验证；不将占位身份当作真实联系人，不宣称其他会话列表同步改用该目录 |
| 个人聊天 HTML 图片嵌入 | `src/toolkit/chat_directory/render.rs` 将本次暂存清单内可用 JPEG/PNG/GIF/WebP 嵌入 base64 data URI。单图 16 MiB、每页图片 URI 64 MiB 预算；预览和原图链接两份均计入。不支持、不可用或超限时提示并保留相对引用；下载链接与音视频仍依赖目录文件 | 嵌入字节/预算/回退专项证据逐项核对及实际导出页面验证；不能宣称所有媒体已成为独立单文件 |
| MCP 配置型本地 Python ASR | `src/cli/mcp.rs` 显式宿主开关 `--configured-local-python` 默认关闭；`src/cli/mcp_voice.rs` 绑定账号后要求配置 `transcription_backend="local"`，读取 `local_whisper_model`（缺省 `base`），构造 `LegacyPythonLocal`。工具请求不能选择引擎；与 whisper.cpp 路径参数、云参数和上传授权互斥，不自动失败回退 | 参数/账号/预算/缓存兼容专项证据逐项核对，以及真实模型、GPU 和识别质量验收 |

Python 依赖的准确边界：仅明确选择可选 `LegacyPythonLocal` 时需要 `src/toolkit/asr/local_python.rs` 的受控 Whisper/PyTorch 推理桥，不启动旧 `mcp_server.py`；账号绑定、语音关联、解码和宿主编排仍在 Rust。该兼容路径仍需 Python、openai-whisper、PyTorch 和模型权重；命名模型可能按需下载权重，不上传音频，不能写成“纯 Rust 模型推理”或“保证全离线”。原生 whisper.cpp 与显式授权云后端是独立选择。直接运行原生 exe 不需要 Node，Node 仅作为可选 npm 安装/启动包装保留；浏览器脚本、Frida 与 WASM 资产另列，不误称为 Node 业务服务。

自动图片与 rich 已完整接线：`web/query.rs` 附加精确图片身份/source，`web/server.rs` 的鉴权 POST 调用 `automatic_image.rs`；`web/assets/app.js` 自动解码、限次重试、取消/释放 Blob，并渲染 `daemon/query/rich_message.rs` 的结构化消息与安全回退。本次 Rust 功能回归及前端 61/0 已验证相应合成场景，不等于正在补充的两项 HTTP→IPC 测试已完成。latency 原生阶段采样已接线，真实环境时延质量没有由合成回归证明。最终精简及整体验收仍进行中；本次文档维护仅更新本文及 `architecture.md`，未另行运行 Cargo、测试或私有数据验证。

## Rules

- Preserve every exposed command, parameter and evidence output unless a behavior change is explicitly documented.
- Keep legacy implementations until Rust equivalents pass contract and fixture comparisons.
- Never replace an unavailable feature with an empty success or silently omit media.
- Test with synthetic fixtures and isolated account workspaces. Never commit keys or chat data.
- Preserve existing outputs on failure. Do not write into source evidence directories.
- Treat Python/Node runtime elimination separately from retaining Frida Hook JavaScript and video WASM assets.

## Module boundaries

- src/cli: argument parsing, command routing, output presentation.
- src/config: active account configuration and runtime location.
- src/scanner: Windows database/account-key capture.
- src/crypto: shared database page authentication/decryption and WAL handling.
- src/attachment: media lookup and image decoding/key providers.
- src/daemon: lazy cached queries and persistent task lifecycle ownership.
- src/service: typed task protocol, fixed settings, execution plans, authenticated IPC and clients.
- src/toolkit: native batch services; databases, images and guarded output operations are separate modules.
- vendor/wechat-decrypt: compatibility implementations and regression reference while migration is incomplete.

## Capability ledger（历史阶段快照）

本表及后续阶段记录保留迁移过程，不是当前未完成事项总表；其中旧 Python 路由或“未接线”判断须重新核对源码。当前范围和本轮三项新增交付状态见上节，禁止从本表推导当前完成率。

| Capability | Current implementation | Acceptance gate |
| --- | --- | --- |
| Memory/account keys | Rust + embedded Frida hook | Existing live 28-database verification and regression tests |
| Queries/search/contacts/groups/favorites | Rust | Preserve existing query tests and output envelopes |
| Raw voice export | Rust | SILK/evidence counts and identity matching |
| decode-transfer | Rust; CLI and initial parity passed | 16 legacy fixtures, shard ambiguity, timestamps, compression, type flags and CLI exit codes |
| toolkit decrypt | Rust; initial parity passed | Page authentication, SQLite quick_check, old/new snapshot comparison, incremental and dry-run behavior |
| toolkit decode-image | Rust; initial parity passed | XOR/V1/V2 byte equivalence and source preservation |
| toolkit decode-images | Rust; initial parity passed | Existing thumbnail layout, skip/force, failure isolation |
| toolkit batch-decrypt-images | Rust; initial parity passed | Mirrored paths, dotted names, thumbnail suffix removal, existing-output skip |
| toolkit export-chats | Python compatibility | All formats, filters, pagination, incremental state and voice options |
| toolkit export-sns | Rust selected-config production routing and timeline update; current automated regression passed | Current all-target baseline: 20 targets, 1142 passed executions, 0 failed, 11 ignored; timeline UI validation pending, not overall migration completion |
| sns-album | Rust account-bound orchestration; targeted CLI regression passed | Existing images/videos, URL fallback, bounded workers, video cache/remote/partial, offline HTML, explicit legacy adoption |
| toolkit voice-to-mp3 / voice-batch | Rust; single-file and explicit-config database batch verified | SILK/PCM equivalence, synthetic SQLite-to-MP3, existing-output skip and individual codec errors; ASR remains separate |
| toolkit transcribe-chat | Python compatibility | Backend/model options, dialect failures, cache/writeback and GPU behavior |
| toolkit run status / -s | Rust; 2 unit tests and 3 real CLI tests passed | Read-only statistics, config-relative paths, transcript truthiness and warnings; real CLI tests with PATH cleared and no Python |
| toolkit export-emoticons / run emoticons | Rust production wiring and prepare safety fixes implemented; current regression passed | 8 toolkit_run_prepare unit tests and 6 real emoticons runtime tests passed; synthetic process-name saved-key path, not real process memory scanning; explicit FFmpeg test passed separately |
| toolkit run aliases (remaining) | Migration incomplete; Python compatibility remains | all and other remaining run workflows still require migration and acceptance; status completion and emoticons wiring are not overall completion |
| toolkit web / gui | Python compatibility at this historical stage | Personal-WeChat HTTP routes, actions, monitoring and desktop workflows; enterprise excluded from current acceptance |
| Video decryption engine | Rust-hosted WASM in decode-sns-video and native sns-album | Node golden byte equivalence, bounded streaming, album cache/remote integration; real CDN/playback remains unverified |
| npm launcher | Node packaging compatibility | Windows launch/exit-code equivalence before removal or replacement |

### 原生相册接线（2026-09-07）

此状态晚于本文后方“相册图片基础层尚未接入”的历史阶段记录。`wx sns-album` 已直接使用 Rust：固定账号后台查询、精确作者绑定、图片/视频工作队列、离线 HTML 和逐文件发布，不再调用 Python/Node。在本相册验收快照中，旧 `toolkit export-sns` 的时间线已有目录更新仍未替换；后续时间线生产接线已有单独的自动化通过记录，见下一节，不能用相册结果替代时间线验收。

- `cli/sns_album.rs` 保留默认 50000 条和六次只读查询尝试，明确 30 秒/256 MiB 单次 IPC 预算；公开 output-root/output-dir、worker、日期和禁用媒体选项。后台返回 `resolved_user/scanned/scan_truncated`；截断进入相册警告。
- `toolkit/sns/album.rs` 编排图片已有文件→原图→缩略图、视频已有文件→完整缓存→远端→部分缓存。视频索引复用现有扫描器但跳过图片缓存，避免图片限额挤占视频扫描。工作线程失败后停止领新任务，已启动线程全部回收再退出。
- `album_images.rs` 与 `album_videos.rs` 使用同一懒加载 Rust WASM 核心，图片上限 25 MiB，视频只解码前 128 KiB 并流式复制尾部，单视频上限 2 GiB。复用已有视频保留 `video_bytes`；缓存文件修改时间跨暂存与最终发布保留。
- `publish.rs` 固定来源绑定和协作写锁，拒绝账号/联系人冲突。无绑定非空目录须显式认领且持续标为 `legacy_unverified`，旧元数据有界读取。保留未知旧文件；媒体、JSON、HTML、摘要逐文件替换，不提供整树事务或跨文件 CAS，失败会报告此前提交数。首次来源绑定不是完成标记。
- 候选数量不另截成 20000；发布前全量检查，逐文件提交只核验树身份、当前源和目标。相同暂存父目录共享守卫，避免随媒体数量平方增长的扫描工作。

最终真实 CLI 12 项回归通过，覆盖绑定更新、显式接管、跨账号/联系人拒绝、图片/两类视频复用、loopback 明文及加密视频、过滤、空结果及参数边界。加密链路使用既有 Node/WASM 合成向量，实际 CLI 不启动 Python/Node，逐字节验证 128 KiB 解密前缀与原样尾部及最终相册引用；完整/部分视频缓存均验证最终 mtime 与源文件不变。修复了 Existing 视频缺失 `video_bytes` 和最终复制丢失 mtime 两项兼容回归。

页面通过 Playwright + 本机 Edge 在 1440×1000、390×844 下检查身份、非空、图片解码、无横向溢出和年份跳转，并回读截图。长英文标题压力测试曾将 1440 像素窗口撑到 2452 像素；仅增加 `h1` 换行规则后，桌面与手机均不再溢出，HTML golden 显式接受这一处样式差异，其余逐字对照旧版。视频 fixture 是空占位文件，其唯一预期资源错误已单列，不声称播放器验收通过。临时页面检查脚本、截图和完整日志保存在 `C:\CodexLocal\wx-cli-album-*`，不提交到产品目录。

最终 `cargo test --target x86_64-pc-windows-msvc -- --test-threads=1` 退出码 0，19 个入口合计 1121 次通过、0 失败、11 忽略。通过数依次为 746、178、7、7、8、6、97、2、1、7、4、7、6、10、3、12、12、4、4；忽略分布为主单元 7、共享生产 harness 2、runtime isolation 2。共享测试会重复执行，合计不是独立用例或功能数量。完整日志为 `C:\CodexLocal\wx-cli-album-complete-final-tests.log`。最终 MSVC `cargo check` 退出码 0，仍有 10 条未使用代码警告，日志为 `C:\CodexLocal\wx-cli-album-complete-final-check.log`；本轮相关文件 `rustfmt --check` 通过。此前 1119 次全量为新增两项测试与标题修复前的历史快照，不替代当前结果。

真实账号加密 CDN、任意视频完整可播放性、全历史覆盖率没有在本轮合成数据验收中得到证明。旧 vendor 仍是其他兼容命令与 WASM 编译输入的来源，尚不进行全目录删除或替换已安装稳定程序。该相册阶段完成不等于旧 SNS 时间线更新、自动导出/ASR、GUI、最终清理或整个迁移目标完成。

### 时间线阶段与当时剩余功能清单（2026-09-07，历史快照）

本节按当时 `src/cli/toolkit.rs`、`src/cli/mod.rs` 及对应宿主源码静态核对，并补录该阶段主集成流程的自动化结果。后续交付不能继续按此清单判定缺失；最新范围及新增补丁状态以本文前方“当前交付增量与验收边界”为准。此处测试数字仅属于当时日志，不覆盖当前集成快照或后续修复补丁。

**阶段边界。** 前一相册阶段的 1121 次通过、0 失败、11 忽略保留为历史快照，不覆盖之后的时间线改动。当前 `toolkit export-sns` 已路由到 `cli/sns_timeline.rs`，调用 `export_database_with_publication`；`export-sns-native` 也已暴露 `--update/--adopt-existing`。当前时间线自动化回归已通过：20 个测试目标合计 1142 次通过、0 失败、11 忽略，主流程确认最终退出码 0。完整日志为 `C:\CodexLocal\wx-cli-sns-timeline-all-retest.log`，本节已核对其 20 条 `test result: ok`；通过数依次为 760、178、7、7、8、6、97、2、1、7、4、7、6、10、3、12、12、4、7、4。各目标含共享测试重复执行，合计不是独立用例或功能数，忽略不计为通过。时间线 UI 验证由另一并行任务负责，当前仍待结果，不能据此宣布整个时间线视觉验收或整体迁移完成。

时间线生产宿主固定选中账号，沿用配置旁 `wechat_files/<账号名>` 派生输出及旧下载环境授权，`--no-remote` 可覆盖授权；生产构造使用 `flat_cache=true` 保留旧 SNS 媒体平铺布局。来源绑定、显式认领、`legacy_unverified` 摘要及失败时已提交数量已进入调用链；这些实现事实不代替产物与失败路径验证。

以下未勾选项表示仍有工作或验收门槛，不表示整项算法都尚未实现。

- [x] **当前时间线自动化回归。** 主流程最终全目标测试通过，结果与日志见上；生产入口、发布核心及 CLI 回归已纳入当前自动化快照，不再标为“仅接线、验证中”。
- [ ] **当前时间线 UI 验证。** 等待负责页面验证的并行任务确认渲染、媒体引用和交互结果；前一相册页面验证不能直接替代本轮时间线 UI 验收。
- [ ] **export-all 与自动转录组合。** `toolkit export-chats` 仍调用 `export_all_chats.py`；`export-chats-native` 已支持日期、增量、`--from-plan-csv/--plan-mode`，不再把计划消费列为缺失，但仍不串联转录。剩余是完整旧参数组合、逐条语音精确关联、转录回写/断点恢复以及公开生产入口替换，不能用分别运行多个原生命令冒充同一工作流兼容。
- [ ] **ASR 后端及真实运行验收。** 已有 `transcribe-audio-native`、显式媒体清单的 `transcribe-chat-native`、单条数据库身份的 `transcribe-database-native`；旧 `toolkit transcribe-chat` 仍调用 Python。本地原生后端要求显式 whisper.cpp 程序/模型，不自动下载，不等价于旧 Python Whisper/PyTorch 配置。仍需明确旧后端迁移策略，完成自动批次关联、真实模型/GPU/云端与失败恢复验收；合成缓存/HTTP 证据不证明识别质量。
- [ ] **一键 export/all、配置及启动器。** `toolkit run status/-s`、`decrypt`、`emoticons`、`decode-images` 已有原生分支；`run export/all` 及其余回退仍走 `run_main`。剩余是前置取钥/解密到导出的完整编排、旧别名与参数/退出码映射、配置向导及默认启动体验。旧 `all` 只提示转录配置，不自动执行 ASR，不应虚构原有能力。
- [ ] **Web、桌面 GUI 与连续监控。** `toolkit web` 和 `gui` 仍分别走旧 main/app_gui。需迁移界面任务、联系人选择、日志/SSE、取消与回收、失败状态、账号隔离和退出重入；daemon/MCP 查询不等于完整持续监控。界面内表情展示也不能用已原生化的表情 CLI 代替验收。
- [ ] **个人聊天多格式媒体目录。** `wx export` 现有 Markdown/TXT/JSON/YAML，完整单聊/批量 JSON 也已有；仍需旧 `export_messages.py` 的 CSV、含图 HTML、`.info` 和媒体相对引用一体化输出及产物对照，不是简单补一个格式名称。
- **企业微信：已排除验收范围。** 旧阶段的取钥、解密、导出及界面兼容缺口不再计为目标未完成项；不继续移植扩展，保留既有代码，必要修复仅维持构建/回归。
- [ ] **独立 SNS 缓存归档。** 时间线媒体恢复和相册不替代旧 `decrypt_sns.py` 的全部缓存图像到 `朋友圈图片/YYYY-MM` 布局；需验证无对应时间线帖子的缓存图片不会遗漏，并保留缩略图选择规则。
- [ ] **持续诊断、图片钥监控与安全清理。** 已有解码、账号取钥和运行统计不等于旧 monitor/latency、独立图片钥监控或 cleanup 的完整用户流程；需逐入口迁移并验证取消、限额、预览及账号/路径所有权，不能复制任意路径删除行为。
- [ ] **真实数据与最终交付。** 相册合成验收不证明真实加密 CDN、任意视频完整可播放或全历史覆盖；还需经授权的真实验收及发布包/稳定安装版本验证。随后才进行全局职责整理、中文注释审校、架构资产同步与逐项删除审计；本次不改架构资产。

**清理条件。** Python 兼容入口仍存在，vendor 还提供生产 WASM 编译输入与回归 oracle，不能整目录删除。Frida 注入 JS、视频 WASM、npm 的 Node 分发包装、ffmpeg/whisper.cpp 外部程序和开发测试脚本应分别审计，不合并成“业务仍全依赖 Python/Node”或“可以全部删除”。

**进度与时间口径。** 当前可报告的是“前一相册阶段已验证；当前时间线自动化回归通过，UI 验证待结果；以上剩余链路尚待迁移或验收”。阶段耗时只能引用实际起止记录，完成预估需有剩余范围与验证成本依据；目前不提供百分比、承诺日期或倒计时，也不把既有测试累计数换算为完成率。

### run status 原生迁移（2026-09-07）

`src/toolkit/run_status.rs` 及其 `tests` 模块已实现 `toolkit run status` 和短别名 `toolkit run -s`；`--` 后支持 `--json`、`--exported-dir`。这是旧一键流程的只读统计入口，原有 `toolkit status` 环境报告保持不变。

统计范围包括配置旁密钥文件的元数据、解密 DB 数量及 bytes、普通导出 JSON 数量及 bytes，以及单会话 `messages` 和多会话 `chats` 两种结构的转录 truthiness。配置中的相对路径以选中的 config 所在目录为基准。消息库 MB 修正为全部消息 DB 实际大小之和；坏转录文件单列 warning，不将不完整统计报告为完整结果。

专项证据：`C:/CodexLocal/wx-cli-run-status-tests.log` 记录 2 项单元测试通过；`C:/CodexLocal/wx-cli-run-status-runtime-tests.log` 记录 3 项真实命令行测试通过。真实测试清空 PATH、无 Python，验证不读取密钥内容、不修改 config/key/transcript、不创建缺失目录。专项通过不代表其他 run 工作流或整体迁移完成；本轮统一回归基线见下节，旧 962 次记录保留为历史快照。

### 表情当前接线与回归（2026-09-07）

`src/cli/export_emoticons.rs` 已接线 `toolkit export-emoticons`（离线使用已保存 key）和 `toolkit run emoticons`（前置微信进程检查及 key 准备）；目录 catalog 与 download 的 Rust 实现已接生产。默认输出改为选中配置旁的 `exported_emoticons`；保留旧单项失败时批次 exit 0 的语义，因此退出码 0 不等于所有单项成功。原生流程不依赖 Python，HEVC 转换可选使用 ffmpeg。

最终前置 prepare 安全修正已完成，`toolkit_run_prepare` 8 项单元测试全部通过：保留已有 keys，不对已有 keys 执行全库 HMAC；新扫描结果验证后原子保存，并拒绝危险路径。

此前专项日志 `C:/CodexLocal/wx-cli-emoticons-runtime-tests.log` 的 5 项通过已由最终统一日志中的 6 项全部通过更新，覆盖真实 loopback HTTP、加密 catalog、无关缺失 key、cache/source-preserve、help、非法 output 及逐项失败 exit 0。第 6 项使用当前测试进程 name 作为 `config.wechat_process`，走真实 `run emoticons` 成功路径并复用 saved keys；未扫描真实进程内存。

最终统一 all-targets 日志 `C:/CodexLocal/wx-cli-status-emoticons-final-tests.log` 共 1127 行，main 确认退出码 0：16 组共 1001 次通过、0 失败、10 忽略；分组通过数为 656、174、7、7、8、6、97、2、1、7、4、7、6、3、12、4，忽略分布为 6、2、2。共享测试重复执行，不把合计当作唯一用例数。既有两条 ASR unused 警告不变；MSVC `cargo check` 及本轮指定代码 `git diff --check` 均由 main 确认退出码 0，日志 `C:/CodexLocal/wx-cli-status-emoticons-final-check.log`。

额外显式运行的 FFmpeg 真实测试为 1 通过、0 失败，日志 `C:/CodexLocal/wx-cli-emoticons-ffmpeg-test.log`；不并入常规全测，基线仍为 1001/0/10，不改写为 1002/0/9。图源与运行总图已同步状态统计和表情实际链；这些证据不代表全部旧功能迁移、真实私有账号或已安装版本验收完成。

## First native batch contract

The four migrated toolkit commands read the active wx-cli account config, not a hidden vendor config. Explicit input/output paths remain CLI-relative; configured output paths are config-relative. Image AES/XOR overrides remain available. Database decrypt preserves the old main-file-only snapshot contract; it does not claim to include live WAL transactions. Live queries still use the daemon.

Native batches emit a JSON summary with total, written, skipped, planned and failures, and return a nonzero exit code for failures. Dry-run performs no writes. Existing outputs are atomically replaced only after successful decoding/validation. Parent-directory path components and output inside a source directory are rejected. This stricter safety behavior is intentional and must be documented in CLI help before release.

## 早期架构待办（历史快照）

- Unify key parsing and account context across CLI, daemon, toolkit and legacy adapters.
- Account-scoped IPC is implemented; continue auditing direct cache consumers and source/cache ownership before broad migration.
- Separate large query parsing/rendering modules without changing message semantics.
- Add golden CLI/output fixtures before migrating bulk chat, SNS, voice and UI services.
- Remove legacy runtime dispatch only after its entire feature surface has replacements.

No old capability is considered migrated merely because another command offers a narrower subset.

## 本轮验证记录（2026-09-06）

- Windows x64 MSVC `cargo check` 通过。
- Rust 全量测试：128 通过，0 失败，2 个原有显式集成/子进程夹具测试按定义忽略。
- 旧 Python 实现：289 通过，3 个子测试通过。必须在 `vendor/wechat-decrypt` 工作目录运行；从仓库根目录运行会使依赖相对路径的打包测试失败。
- 差异测试：XOR/V1/V2 字节、普通批量目录、缩略图后缀及去重、重复运行跳过、错误 XOR 保留旧结果全部通过。
- 真实账号的隔离数据库副本：新旧解密逐字节相同，预览不写入、增量跳过、错误密钥保留旧文件通过。私有数据与密钥未加入仓库。
- 修复了差异测试发现的普通批量图片误覆盖问题，以及 Clap 互斥参数导致的调试构建崩溃。
- 架构 SVG 的几何及布局检查通过；使用本机 Edge 渲染 PNG，目视检查通过。详细说明见 [系统架构](architecture.md)。

这只是首批原生能力和兼容层拆分的验收，不是整个系统迁移完成证明。
尚未覆盖已安装稳定版；尚未验证本轮发布包、所有真实媒体格式以及 GPU ASR 的端到端迁移。

后续优先处理账号运行上下文与后台身份隔离，再分阶段迁移批量聊天、朋友圈、语音和界面。
新增或重构处的注释尽量使用中文；不修改外部协议字段、第三方代码版权声明及用户内容。

## 账号运行层重构

- 已新增 `src/runtime.rs`，统一账号身份、管道、缓存、PID、日志和锁目录。
- 已替换固定全局管道，后台启动固定配置文件并验证预期身份。
- 已增加启动串行锁和后台存活锁；停止时校验 PID 创建时间，防止误停复用的 PID。
- 已增加 Rust 真实进程集成测试，使用合成数据库验证账号隔离、并发启动及失败清理。
- 原有稳定版尚未被覆盖；旧缓存和旧后台未自动删除或接管。
- 本阶段最终验证：Windows x64 编译通过，133 项单元测试及 4 项真实进程集成测试通过，0 失败，2 项原有显式测试忽略。
- 静默管道连接有可取消超时；身份记录篡改拒绝停止、陈旧记录重启、自定义运行目录配置发现均有回归测试。
- 六组数据库/图片差异测试在账号运行层接入后通过；之后的管道期限修改不改变这些直连批处理命令的实现。

## 原始功能移植分组（历史，企微已排除）

| 分组 | 旧源码入口 | 不可遗漏的内容 |
| --- | --- | --- |
| 配置与启动 | config / setup / main / launcher / cleanup | 配置默认值、依赖诊断、启动别名、清理预览与安全边界 |
| 微信密钥 | find_all_keys / key_scan_common / key_utils | 格式兼容、候选校验、多数据库及账号派生 |
| 图片 | decode_image / batch_decrypt_images / find_image_key / monitor | 附件定位、V1/V2/XOR、图片密钥及动态监控 |
| 聊天导出 | export_all_chats / export_chat / export_messages / chat_export_helpers | 选择与过滤、分页、增量、输出布局和语音回写 |
| 朋友圈 | decrypt_sns / export_sns / export_sns_album / sns_media_wasm | 时间线、缓存、图片回退、并发补图、视频恢复、相册 |
| 表情 | emoticons / export_emoticons | 本地缓存、收藏表情、素材格式和下载 |
| 语音 | voice_to_mp3 / wx_toolkit_voice_to_mp3 / transcribe_chat | SILK 编解码、ASR 后端、缓存、失败处理、模型配置 |
| 服务与监控 | mcp_server / monitor / monitor_web / app_gui / latency_test | MCP 工具、消息类型语义、实时监控、HTTP 路由、桌面工作流、性能诊断 |
| 企业微信 | find_wxwork_keys / wxwork_crypto / decrypt_wxwork_db / export_wxwork_messages | 独立密钥实现、数据库解密、消息及联系人导出 |
| 转账解析 | decode_transfer | 二进制/结构化字段解码和金额语义 |

模块内部可重构合并，不要求逐函数机械翻译，但旧能力必须能从新接口到达，并通过对应回归测试。

## 转账移植映射

| 原实现 | 新实现 | 验证 |
| --- | --- | --- |
| decode_transfer.py 命令入口 | cli/decode.rs + DecodeTransfer CLI 参数 | 真实 CLI 退出码 0/1/2、可选时间戳、JSON |
| mcp_server._extract_transfer_info | message/transfer.rs | 16 组旧实现对照样本 |
| mcp_server._format_transfer_message_text | message/transfer.rs + query 摘要调用 | 字段、摘要、缺失金额、未知状态 |
| mcp_server.decode_transfer 查询与展示 | daemon/query/decode.rs + message/transfer.rs | 多分片冲突、压缩、高位消息类型、结构化信息 |

旧 MCP 服务的整体移植仍未完成；上表不代表 mcp_server.py 全部能力已经迁移。

本阶段验证：Windows x64 编译通过，139 项 Rust 单元测试与 5 项进程集成测试通过，2 项原有测试忽略；旧 Python 集合 289 项与 3 个子测试通过。
本阶段没有覆盖安装版，也没有提交或移除旧生产代码。接下来继续移植消息语义、批量导出、媒体及服务；全部迁移完成后还有原有代码重构与项目清理阶段。

## 语音、名片与位置阅读摘要

`message/summary.rs` 承接旧 `_format_voice_text`、`_format_namecard_text` 和 `_format_location_text` 的阅读摘要，由查询层在剥离群聊发送者前缀后调用。语音保留时长；名片仅展示名称和简介，不输出认证字段；位置展示类别、地点和地址。原始存储不修改，消息标识仍由查询结果的结构化字段携带。

解析器保留旧版 20,000 字符限制并拒绝 DTD/实体声明。`tests/generate_summary_golden.py` 只提取旧代码纯函数，生成 27 组无私人数据的对照样本；Rust 测试覆盖对照输出、损坏 XML、长度边界、群聊前缀与高位消息类型。

本次仅迁移阅读摘要，不代表 `decode_location` 的结构化坐标、电话、营业时间等字段和命令入口已迁移。旧生产实现继续保留；不得因此删除旧模块。

后续进展：`message/location.rs` 已承接结构化解析，完整保留旧版 17 个文本字段以及 lat、lng、category_top。摘要改为由该对象生成，XML 安全边界抽到 `message/xml.rs`。缺失文本为空串、缺失或非法坐标为 null；NaN/Infinity 也规范化为 null，以保证标准 JSON 有效，原始 XML 不修改。已有阅读摘要对照样本继续约束显示行为。

详细 `decode-location` CLI、跨分片定位及详细文本渲染尚待迁移。当前不能将整个位置解码功能标记为完成。

后续命令接入：`wx decode-location <chat> <local_id> [create_time] [--json]` 已加入 Rust CLI/IPC/后台分发，详细文本由 `Location::render` 生成。位置与转账共用 `daemon/query/decode.rs` 的只读多分片扫描、唯一性检查及解压缩；输出共用 `cli/decode.rs`，错误码为 1、歧义为 2。查询测试覆盖位置跨分片冲突、时间戳选取、压缩、高位类型、群聊前缀和损坏内容。旧 MCP 服务整体仍未迁移，不能删除旧服务文件。

位置验证补充：新增 11 组旧实现结构化字段及详细文本对照样本。真实 CLI 进程测试使用合成加密双分片，覆盖位置 JSON、文本、时间戳选取、歧义与错误类型，且运行环境指定不可用的 Python 路径。该测试发现并修复了 `source` 返回解密缓存哈希文件名的问题：解码查询现在保留原始分片来源，转账和位置同时受益。旧 Python 回归集 289 项与 3 个子测试通过。

## 通话状态及群聊前缀

`_format_voip_message_text` 已迁入 `message/summary.rs` 并接入 type=50 查询摘要。已知英文状态按旧规则翻译，时长保留原字符串，未知状态原样保留；不推断通话媒介或虚构接听状态。摘要对照样本增加到 44 组，其中 17 组覆盖通话；位置详细对照仍为 11 组。

`message::split_group_content` 统一剥离旧版支持的 `sender:\n` 和无换行 XML 前缀，摘要与详细消息解码共用。无换行时严格限制账号字符和 XML 前缀，普通 URL、中文说明、无发送者及未知标签不剥离。此项不代表所有群聊发送者身份解析调用点已经完成统一审计。

## 导出辅助规则迁移

`chat_export_helpers._format_video_message` 已迁入 `message::summary::video` 并接入 type=43 查询。播放时长字符串原样保留，空值或损坏 XML 回退为视频标记；新增 10 组旧实现对照，摘要样本合计 54 组。此项仅处理文字摘要，不代表视频文件导出或整个聊天导出入口已经迁移。

现有群成员 protobuf varint 解析补齐第十字节溢出校验，覆盖 u64 最大值、溢出、截断、非法起始偏移和正常多字节值。表情说明的 protobuf 布局仍需验证，尚未迁移；旧实现继续保留。

## 查询完整性基础修复

核对单聊导出入口时发现，历史查询及搜索使用 `filter_map(Result::ok)` 静默丢弃损坏行。两处消息行收集现改为传播错误；搜索外层不再把失败表或失败分片包装成部分成功，等待已启动分片任务结束后返回错误。新增损坏行回归，避免将漏行结果当成完整证据。

群聊发送者 username/label 回退统一使用 `split_group_content`，修复无换行 XML 正文可解析但发送者为空的问题；有效 Name2Id 映射仍优先于正文前缀。新增优先级测试。

本轮没有完成 `export_chat.py` 原生命令替换。统计、收藏等其余容错路径仍需独立审计，不能将这两处修复解读为全系统完整性已经验证。

## 单聊导出格式层（历史接线阶段）

`message/export.rs` 定义紧凑消息及聊天文档模型：文本省略 type，None 正文省略而空正文保留；缺失时间保持 null，排序按零处理且同时间戳稳定；未知类型保留完整原类型值；一对一省略 is_group；扩展字段可细化类型但不得覆盖身份字段。新增测试覆盖这些边界。

该模块尚未接入生产扫描或 CLI，因此当前存在 6 条未使用警告，未用 allow 属性隐藏。旧实现的自动格式对照、转账扩展构造、分片扫描与发送者映射接入仍待完成；不能据此标记 export_chat 已迁移，也不替换旧命令输出。

后续进展：`export-chat <chat> <output.json>` 迁移预览入口已接入 `cli/export_chat.rs → IPC ExportChat → query/export.rs → message/export.rs`，模型不再有未使用警告。逐分片读取 Name2Id、消息行、压缩正文，携带 source 并稳定排序；未知分片、行类型异常及解压错误会失败。CLI 原子写出，禁止覆盖数据库目录、当前配置和密钥文件，查询失败不覆盖已有输出。

预览入口还不等价于旧 `export_chat.py`：目前使用查询摘要，转账结构化 extras、本人 me 识别、部分消息正文省略规则和表情说明尚未对齐；全空时间戳表的发现以及跨数据库一致快照仍待审计。不能把该预览作为正式 AI-ready 更新入口。原 `export`（分页、多格式）及 Python 批量导出均保留。

合成加密双分片集成测试已加入预览导出的记录数、顺序、来源、失败保留和源目录/密钥保护。并行测试临时目录改用时间戳加原子序号、独占目录创建，避免仅靠时钟导致夹具混用。

导出发现逻辑已独立于分页查询的 MAX(create_time)：按已知分片逐个检查目标表是否存在，空表可输出空消息数组，全 NULL 时间戳记录保留 null，不再漏掉整张表。新增合成加密数据库 CLI 测试覆盖这两种场景。分片路径按键名稳定排序，已知分片无法加载明确失败。

每个分片内使用读取事务，使 Name2Id 与消息行来自同一读取快照；这不等于多个数据库的统一时点快照。跨数据库一致性及旧导出内容语义对齐仍未完成，预览状态保持不变。

发送者语义已迁入 `message/identity.rs` 并接入原生导出：账号目录候选必须命中联系人用户名才识别本人，优先去除至少四位十六进制后缀，不按昵称猜测；本人输出 me。群聊 Name2Id 优先、正文发送者回退；一对一对方使用聊天显示名，未知归属不猜成本人。Name2Id 的 NULL/空用户名占位行按旧实现忽略，其他读取错误仍传播。40 组旧 Python 纯函数对照验证身份组合，另有目录候选和优先级测试。

## 并行移植与命令接入（2026-09-07）

四个独立实现方向同时推进：语音、朋友圈、聊天正文、企业微信。公共 Cargo、CLI、IPC、集成测试和文档由主流程统一修改。下列记录仅描述实际接入，不把派发任务计为完成。

| 能力 | 现有 Rust 入口 | 本次证据与边界 |
| --- | --- | --- |
| 一键图片别名 | `toolkit run decode-images -- ...` | 复用直接原生命令参数；Python 不可用时实际解密、帮助及错误参数进程测试 |
| 单文件 SILK 转 MP3 | `toolkit voice-to-mp3 input.silk [output.mp3]` | silk-codec 0.3.1 内置 SDK 解码，ffmpeg 编码；5 组纯合成 PCM 与 pilk 逐字节对照，源文件别名及失败回滚；不代表数据库批量与 ASR 完成 |
| 企业微信离线解密 | `toolkit decrypt-enterprise input.db output.db --key-file raw-key.txt` | 4096 字节页、头片段与整页 AES128 布局，6 页合成数据库逐页/文件对照、SQLite integrity_check；输出拒绝覆盖，拒绝 WAL/SHM/journal，无 MAC 不能宣称密码学认证 |
| 正文兼容 | `message/export_content.rs` 已接入单聊及批量预览 | 259 个旧版 AST 差分样本及边界样本；新增 app 19/57/51/2001，引用传入联系人及本人上下文；移除临时摘要降级，NULL/空串及正文省略保留 |
| 原生全量批次预览 | `toolkit export-chats-native output_dir [--users ...] [--start ...] [--end ...] [--dry-run]` | 全部 SessionTable 会话、不按 last_timestamp 过滤，按精确 username 导出；日期按本地时间或 Unix 秒，含两端点，读取后筛选；首末消息日期重算；尚无计划 CSV、增量合并、转录和完整联系人元数据 |
| 导出文件索引 | `toolkit/chat_index.rs` | 兼容旧 `_export_index.json`；流式读取 JSON 身份；同名隔离、未知文件不覆盖、批次互斥；备注变化后新文件发布再记 previous_files，旧文件保留，不在导出前移动旧文件 |
| SNS 预览 | `toolkit export-sns-native sns.db output_dir [--contact-db ...] [--contacts ...] [--utc-offset +08:00] [--xwechat-cache ...] [--sns-cache ...] [--download-media]` | 38 组解析对照、旧导出函数合成数据库对照；动态与评论同一读取事务，联系人目录整体发布；可选显式缓存图片/MP4 恢复，32 组 DAT 向量和本地引用进程测试；默认离线，仅显式开关补下载缓存未恢复项，引用使用真实扩展名；图片匹配为启发式，已有 SNS 目录仍拒绝覆盖 |

旧 `toolkit export-chats`、朋友圈完整导出、MCP、Web/GUI、Node/WASM 等未完成入口继续保留。原生语音批处理现可通过显式配置使用，但旧一键工作流尚未整体替换。原生聊天批量入口明确为迁移预览，不能作为已完成全功能迁移的证明。全功能迁移后的原有 Rust 架构整理和最终清理仍属于目标。

本批主仓验证：Windows x64 MSVC `cargo check` 无警告；189 项 Rust 单元测试、10 项默认进程测试通过。另显式运行 ffmpeg 五组 PCM/MP3 逐字节对照与“不存在 Python 路径”的语音 CLI 测试，均通过。旧 Python 集合 289 项及 3 个子测试通过。默认忽略项仍含原有 Frida 冒烟、子进程夹具以及需 ffmpeg 的音频用例，不把忽略计作通过。

批量导出集成测试曾发现新会话列表返回顶层数组不满足 IPC `flatten` 对象契约，已改为 `{ "chats": [...] }` 并复跑通过。同名联系人与不在通讯录中的 username 均按真实身份导出；`NULL` 正文、空字符串和图片正文省略规则已通过进程测试。SNS 进程测试覆盖离线输出、固定时区、重复运行拒绝覆盖、源数据库不变和临时目录清理；企业微信命令测试覆盖两种加密布局与不回显密钥。

### 第二批集成验证（2026-09-07）

- 企业查询 `toolkit/enterprise/queries.rs` 及内容/导出子模块接入 `toolkit enterprise`：只读事务、三表去重、联系人和群名片、消息过滤分页、JSON/CSV/HTML 导出。七组 Rust 测试对照旧实现合成素材，进程测试验证 4 联系人、4 会话、7 条去重消息、三种输出、源文件不变和重复导出拒绝覆盖。发现并移除了个人微信配置校验依赖，离线企业操作不要求个人账号配置。
- 语音 `toolkit/audio/batch.rs` 接入 `toolkit voice-batch --config ...`：配置相对路径、精确联系人筛选、同名目录身份、`.info` 独占发布、MP3 不覆盖、逐条失败。真实 ffmpeg 集成覆盖 5 行合成 SQLite、3 条实际转换、1 条已有跳过、1 条损坏；重复运行跳过 4 条，数据库字节不变。独立 CLI 测试验证明确配置、不存在 Python、生成 MP3 及重复运行跳过。
- 日期筛选覆盖四种旧日期格式、Unix 秒、非法日期及反向范围、包含端点、NULL 时间以及空范围；进程测试验证反向范围不创建输出，2 至 3 秒范围精确导出每会话两条消息。
- 本次完整主仓回归：209 个单元测试、11 个默认进程测试通过；单元测试 4 项及新增语音 CLI 后的进程测试 2 项默认忽略。批量音频的单元/CLI 两项已使用 ffmpeg 显式运行通过。此前的单文件音频逐字节测试仍保留；新测试结果不替代完整功能验收。
- Windows x64 MSVC 编译无警告。没有替换稳定安装二进制、删除旧生产脚本、提交或推送。

### SNS 缓存接入与最终优化范围

`sns/cache.rs` 已接入导出暂存目录：XOR/V1/V2 图片、按尺寸/大小/时间窗匹配图片、按帖子与媒体 ID 查找本地视频。只访问显式目录；密钥文件仅在 CLI 读取，密钥内存使用后清零；缓存源变化、输出冲突等失败明确记录。图片匹配不是确定身份关联，MP4 的 complete 标记仅沿用扩展名和头部规则，不等于完整可播放性验证。网络回退、Node/WASM 视频和完整相册仍待迁移。

缓存开启时，XML 媒体 ID 经内部字段传到恢复器，不改变默认纯文本 JSON。恢复后的本地路径和视频布尔字段同时写入单条 JSON 与时间线；HTML 只使用本次生成的本地引用。`_media_recovery.json` 记录每项恢复/缺失/失败状态。合成 CLI 测试验证 XOR PNG 恢复字节、MP4 缓存复制、媒体 ID、两份 JSON 相等、本地 HTML 引用、源库及缓存字节不变、重复输出拒绝覆盖。

缓存接入完整回归为 221 个单元测试、11 个默认进程测试通过；随后新增引用进程测试的 SNS 专项回归为 47 单元、2 进程通过，1 项符号链接权限测试忽略。忽略项不是通过证据。

用户新增第 8 项验收要求：完成全部迁移及回归后，再对全仓模块和代码做最终合并、删除和精简，尽可能以更少代码实现同等功能。该阶段尚未完成，不用提前删除旧生产代码代替验收。

### 十路并行与主仓验证

用户要求十个子任务并行开发和回归；运行环境实际最多六个同时运行，采用完成后补位，总共派发十个独立分工：增量合并、delta、计划统计、联系人元数据、本地 ASR、OpenAI 兼容 ASR、转录回写、WASM 视频、MCP 协议和专项安全回归。独立模块交付不自动计为主仓已接入。

本批已接入 `toolkit/chat_merge.rs` 和 `toolkit/contact_metadata.rs`。增量按 source/local_id 去重，旧消息及转录不覆盖，日期只过滤新增，索引按 username 定位旧文件；旧文件损坏或来源未知的同号碰撞明确失败。联系人查询从独立实现的 message 路径移至 toolkit，字段和标签共用只读事务；四个旧字段以 108 个 AST oracle 样本验证，诊断独立输出。进程测试验证重复增量为零新增、旧字段保留、歧义失败不覆盖。

专项安全回归发现语音批处理先调用 absolute 导致 `..` 检查失效。已改为检查原始输出路径后再归一化，保留“任何目录创建之前拒绝”的进程测试。修复后主仓 MSVC check 无警告，完整回归：230 单元、6 专项安全、12 进程测试通过；5 单元和2 音频进程测试默认忽略。独立 ASR、WASM、MCP、delta、计划模块仍须逐一接线及主仓验收，不将其独立测试结果混入此统计。

### 原生 ASR 与 WASM 接入（2026-09-07）

新增 `toolkit decode-sns-video`：Rust wasmi 0.46.0 承载同一已审计 WASM，模块内嵌并校验 SHA-256，不启动 Node。文件入口只解码前 128 KiB，再流式复制尾部，同目录临时文件同步后不覆盖发布。保留 35 组 Node 密钥流字节对照、燃料/内存/密钥限制及错误后实例隔离测试，新增源文件不变、尾部保留、密钥失败不创建输出等用例。只验证 MP4 头部，不替代完整播放校验或相册下载流程。

新增 `toolkit transcribe-audio-native` 与 `transcribe-chat-native`：SILK 原生解码至 WAV；本地 whisper.cpp 或明确授权的 OpenAI 兼容端点。无默认云配置、无隐式上传或模型下载；云端禁用重定向和环境代理，限制请求及响应大小。聊天媒体清单按 username/source/local_id 精确关联，已有转录保留、逐条错误独立报告，批次结束回写。真实模型、GPU、转录缓存、数据库自动关联和旧一键工作流替换仍未完成。

本地后端新增临时输出大小、条目数及 stdout/stderr 限额，Job Object 回收已纳管派生进程。磁盘轮询可能短暂超调，spawn 到纳管仍有窗口，不是恶意程序沙箱。安全回归发现转录期间目标被其他账号文件替换后会遭覆盖；现以协作锁、文件身份/完整字节/修改时间复核和新文件不覆盖发布处理。Windows 原地更新与硬链接回归验证通过。关闭快照句柄至最终替换仍有非协作写入窗口，详见 `src/toolkit/asr/WRITEBACK.md`；不宣称严格 CAS。

本次主仓 MSVC `cargo check --target x86_64-pc-windows-msvc` 无警告；完整 `cargo test`：288 单元、81 ASR/视频 harness、6 迁移安全、12 进程测试通过。分别忽略 5、2、0、2 项；harness 会复跑部分生产模块测试，不将各组总和当成独立覆盖数量。曾失败的竞态和 Windows 正向更新用例保持启用。完整本机日志为 `C:/CodexLocal/wx-cli-asr-video-final-check.log` 与 `C:/CodexLocal/wx-cli-asr-video-final-tests.log`。

三张新架构图已按源码核对、渲染并回读：[运行总图](diagrams/runtime-current.svg)、[SNS 缓存发布](diagrams/sns-cache-publish.svg)、[ASR/WASM 边界](diagrams/asr-wasm-wiring.svg)。保留源码证据及复现脚本。MCP、完整 delta、计划 CLI 和数据库语音关联继续并行接入；当前不删除旧运行时，不替换已安装稳定程序，也不代表整体重构或最终精简完成。

### MCP、Delta、计划与数据库语音入口（2026-09-07）

- `wx mcp` 已注册原生 stdio 协议及固定账号 IPC 适配器，提供八个真实只读工具。启动时锁定显式账号配置；错误使用固定公开类别，输入、输出及 IPC 响应均有限长。健康探测响应也限制为 1024 字节，不能绕过 MCP 的读取限制。同步 stdio 仍不能在回调执行期间读取取消通知，后台启动有独立超时；其余九个旧工具尚未全部接入，详见 `src/mcp/PROTOCOL.md`。
- `toolkit export-delta-native` 从账号级后台按精确 username 读取原始消息，保留正文原始字节和来源用于 UID，输出 run 与清单。默认独占新建 root；显式 `--append-run` 可在已有 root 中独占创建新 run，不修改旧完整聊天或已有 run。整个批次固定一个 RuntimeContext，拒绝数据库、解密缓存、运行目录及配置/密钥路径作为输出。
- `toolkit chat-plan-native` 使用显式离线数据库生成兼容计划 CSV，可选择估算或扫描媒体大小。扫描统计实际目录条目而不是按消息日期推断媒体大小。解密根及其祖先在数据库打开前检查并保持路径保护，拒绝祖先 junction 绕过；扫描错误保留明确的状态，不把部分扫描伪装为完整结果。
- `toolkit transcribe-database-native` 通过显式 username、message 分片及 local_id 定位消息，再严格关联媒体库 Name2Id/VoiceInfo，SILK 字节直接进入已有 WAV/ASR 管线，不生成中间 SILK 文件。歧义、缺失、媒体分片变化或模式错误拒绝。调用者提供的账号来源不等于认证，证据中明确 `account_authenticated=false`；本地模型、GPU 和真实云转录仍未验收。

本地模拟 HTTP 服务在 Windows 下显式恢复 accepted socket 的阻塞模式，并在原有总期限内处理 Interrupted/WouldBlock；不放宽生产超时。MCP 安全测试改用真实 `ping -> ping -> contacts` 序列验证超大初始 Pong 被拒绝后重试成功，不再以服务端写入成功推断客户端内存行为。

新增 `VoiceMessages` 只读 IPC：媒体键由账号级 DbCache 枚举，原始键用于查找，规范来源用于跨片排序与证据。返回 voices/count 对象、真实 length(voice_data)，NULL 保持 null。真实加密数据库进程测试覆盖大小写与反斜杠键、全局分页、缺片/未知片失败、两个账号同 ID 隔离及源文件不变。尚未注册 MCP 的 `get_voice_messages` 工具。

独立语音审查复现并修复两处问题：列表查询拒绝普通或生成列遮蔽 SQLite rowid，亦拒绝视图、虚拟表及 WITHOUT ROWID；数据库 ASR 枚举识别大写媒体文件，保留磁盘原名，拒绝规范来源冲突及文件别名。ASR 的模式检查同样使用 table_xinfo 覆盖生成列。独立 45 项联合测试现在断言拒绝原始反例，并保留源字节不变检查；不以“复现漏洞的测试通过”代替修复验证。

修复后 Windows x64 MSVC 完整回归无警告：主仓 407 单元、54 默认集成/进程测试通过；ASR/视频 harness 114 项通过，其中重复包含生产测试，不相加为唯一覆盖数。分别忽略 5、2、2 项（主仓、普通进程、ASR/视频 harness）；忽略不是通过。旧 Python 回归 289 项及 3 个子测试通过。完整本机日志为 `C:/CodexLocal/wx-cli-voice-fixes-full-tests.log`、`C:/CodexLocal/wx-cli-expanded-legacy-pytest.log`，`git diff --check` 无输出且不再出现 LF/CRLF 提示。全仓 rustfmt 检查仍有历史模块格式差异，本轮仅格式化实际触及的入口文件。

标签查询、结构化引用解析和转录缓存仍有独立实现、复用精简或接线工作，不能按模块文件存在即计为已公开能力。已开放仓库内部 XML、引用摘要和联系人字段解析接口，供新模块复用；删除重复实现及对应回归由各模块继续完成。旧生产脚本、稳定安装二进制及完整旧工作流继续保留；全量功能替换、全仓架构整理和最终删除精简尚未完成。

### 十二项 MCP 工具与显式转录缓存（2026-09-07）

本节更新上一阶段的八工具状态。`get_contact_tags`、`get_tag_members`、`decode_refer`、`get_voice_messages` 已加入原生 MCP，工具总数为十二。标签和引用复用已有联系人元数据、XML 与摘要实现，删除各自重复逻辑。标签列表只投影名称及成员数，成员需单独查询；引用定位先检查完整分片清单和唯一性，再检查消息类型，不能用类型过滤隐藏同 ID 冲突。严格清单识别大写文件名，读取错误不能退化为空清单；同名视图、虚拟表及 WITHOUT ROWID 明确拒绝。

新增真实加密数据库到 daemon、IPC、MCP 的进程测试，覆盖这四个工具、两个账号同 ID 隔离、引用歧义、NULL 语音长度以及五个未注册写工具拒绝。对照源文件逐字节不变，不仅依靠模拟 IPC 返回值。

数据库单条语音 CLI 可显式成对指定 `--cache-file` 和 `--cache-account`。默认无缓存，账号命名空间不代表认证；云端命中仍检查授权，本地模型与程序按内容识别，成功空串可命中，失败不持久化，缓存故障不抹去转录成功。缓存不提供多写入者 CAS，云模型别名不保证服务端内容不变。独立审计发现源数据库外部硬链接未被路径预检拒绝，已进入修复，尚不能将该边界记为验收通过；复现未证明源数据破坏。

图片解码器修正 `V2KeyMaterial::default()` 的 XOR 默认值为 `0x88`，同时保留显式 `0x00` 或其他账号值。合成 V1 图片测试逐字节验证 AES 头、原始中段和 XOR 尾部。云缓存回环测试修复 Windows accepted socket 非阻塞继承，在固定期限内重试，不调整生产超时。

本阶段 `cargo test --all-targets` 退出码 0：455 主程序单元、131 ASR/视频 harness、55 其余集成/进程测试通过；分别忽略 5、2、2 项。各目标会重复执行部分生产模块测试，不把合计 641 次通过当作唯一用例覆盖数。完整输出位于 `C:/CodexLocal/wx-cli-twelve-all-targets.log`。普通二进制仍有缓存 API 的两条未使用警告，另行精简处理；不能沿用上阶段“无警告”结论。本轮尚未重跑旧 Python 全集。

五个 MCP 工具 `decode_image`、`decode_file_message`、`decode_record_item`、`decode_voice`、`transcribe_voice` 尚未公开接线。附件适配、共享严格定位、完整 History 多类型与最早页、联系人旧字段覆盖，以及旧工作流整体替换和最终删除精简继续进行。历史架构位图尚未更新至十二工具状态；源码 Mermaid 入口与协议文档已同步，不能将旧图视为最新验收证据。

### 十四项 MCP 工具与共享附件查询（2026-09-07）

本节取代上一阶段的工具缺口和缓存待修复状态。`decode_file_message`、`decode_record_item` 已接通协议、IPC、daemon 和真实本地缓存查询，公开工具总数为十四。新增 `query/mcp_attachments` 只负责流程适配，元数据和查找使用 `toolkit/attachment_refs`；引用和附件的唯一消息定位、有界解压统一到 `query/strict_message`，不再各自复制跨分片查询。保留先检查完整身份再检查类型或条目索引的顺序。

附件根来自当前固定账号的数据库配置，不通过工具参数、消息 XML 或默认账号猜测。只返回原始副本引用；不写入、下载或解码图片、语音、视频，与旧 record 工具的本地引用契约一致。状态区分已找到、缺失、文本及仅元数据；无消息 MD5 的单个候选保留弱匹配警告，多候选不任取。外层文件 20,000 字节与转发记录 500,000 字节解析限额分别保留，不沿用 History 的条目截断。

真实加密数据库进程测试现在同时覆盖六项新增只读工具。新增附件验证检查路径所属账号、实际文件字节及 MD5、另一个账号独有文件不能被当前账号找到、同 ID 跨分片与错误类型行仍算歧义、61 项大记录及第 60 项可读取，以及源文件逐字节不变。独立附件夹具 68 项通过，主仓注册后的附件专项为 47 项通过，两者包含范围不同，不能混用统计。

上一阶段缓存外部硬链接问题已修复：受限枚举消息、媒体及固定联系人源后按文件身份拒绝别名，不递归、不跟随链接，授权顺序不变。原红测试保持断言并转为通过，独立安全夹具无过滤运行 137 项通过、3 项忽略。删除仅测试使用的 `store_result` 和 `FailureSkipped`，失败不缓存通过真实转录后端路径验证；缓存格式与 CLI 输出不变。

Windows 主仓 `cargo check --bin wx` 和 `cargo test --all-targets` 均退出 0、无警告。全目标结果为 509 单元、135 ASR/视频 harness、55 其余进程/集成测试通过，分别忽略 5、2、2 项；合计 699 次通过不代表唯一用例数。旧 Python 全集重跑 289 项及 3 个子测试通过。完整本机日志：`C:/CodexLocal/wx-cli-fourteen-all-targets-final.log`、`C:/CodexLocal/wx-cli-fourteen-legacy-pytest.log`；首次全目标运行曾因真实 MCP 测试仍断言十二工具而失败，日志另存 `wx-cli-fourteen-all-targets.log`，没有删除失败记录。

剩余三个旧 MCP 工具为 `decode_image`、`decode_voice`、`transcribe_voice`。图片核心独立 43 项通过但尚未公开接线，不能计为第十五项。图片/语音查询适配、完整历史分页与类型筛选、联系人旧字段兼容，以及静态架构图更新仍在进行。旧生产实现、稳定安装程序和全量工作流保留；整体迁移、最终合并删除与最终验收尚未完成。

### 历史多类型分页与附件标量校验（2026-09-07）

`get_chat_history` 已支持旧参数 `msg_types` 的多类型并集和 `oldest_first=true`，由 IPC 到 `q_history` 实际执行。CLI 新增 `--types text,image` 与 `--oldest-first`，保留旧 `--type`；非空新旧类型参数互斥，旧导出命令不启用最早页。每分片先应用类型与闭区间时间过滤并取 `offset+limit` 候选，再全局选页；两种方向最终均按时间升序展示，不以反转最新页冒充最早页。

新增 `query/history_selection` 专注选行和分页，旧单类型路径保留原过滤规则；两条路径共享 `read_history_row` 与 `render_history_rows`，正文、发送者、群昵称和 URL 不再复制实现。参数预检在联系人查询前完成，拒绝无效范围、溢出及超长类型列表。同时间排序不保证跨快照稳定游标；旧 Python 渲染失败后继续补取候选的行为仍是差异，未声称完全逐字兼容。

助手经 288 组旧 AST 与真实 SQLite 对照验证，既有 query 单测 103 项通过。主真实加密账号夹具新增跨两个分片交错的时间/类型，分别验证最早页、最新页、偏移、时间窗、高位类型、默认旧 IPC，以及实际 `wx history --types ... --oldest-first` 调用；两个账号正文和源文件保持隔离。

独立附件审计发现结构化 MD5/大小字段会降级为空，导致错误附件按弱匹配返回。现只对这些标量字段拒绝子元素，合并合法文本/CDATA/注释分段；保留真正空值与富文本描述。原五条失败断言不改动，安全夹具 21 项与主仓附件 50 项全部通过，实际 SQL 反例不再返回错误文件。

为后续语音入口增加 `database_media` 显式逻辑源/解密路径清单及精确时间接口，与旧目录入口共用关联算法；`DbCache::raw_db_keys` 返回原始键名，不包含密钥，现媒体枚举复用该方法。`mcp_audio` 返回原始 SILK 与证据，真实缓存夹具 86 项通过，但尚未注册 MCP 语音解码或转录工具。`mcp_image` 已改用相同原始键接口，独立 13 项通过，仍须接入宿主输出目录与显式密钥配置。

当前 Windows 全目标回归退出码 0、无编译警告：517 主程序单元、139 ASR/视频 harness、86 语音适配 harness、55 其余进程/集成测试通过，合计 797 次执行；分别忽略 5、2、0、2 项，不将共享测试重复计为唯一覆盖。完整日志 `C:/CodexLocal/wx-cli-history-scalar-final-tests.log`。旧 Python 全集在上一阶段为 289 项及 3 个子测试通过，本轮未修改旧实现或重跑该全集。差异检查在本轮导出文件行尾统一后退出 0、无输出。

三张静态 SVG/PNG 已更新到十四工具、共享附件定位、显式 ASR 缓存及增量追加状态，历史分页连线也已同步并回读。图片与语音尚未公开的链路保持待接线标记。公开 MCP 数量仍是十四；联系人完整旧视图、三个写工具、剩余旧工作流、最终整理删除及整体验收继续进行。

### SNS 显式下载与后台生命周期回归（2026-09-07）

本节是后续阶段快照，前面各阶段的工具数量、测试统计和待接线描述只代表当时状态。当前 MCP 已有十七项工具，详见运行架构图；本轮新增的是 `export-sns-native --download-media`，不是完整旧相册编排。

时间线复用原有导出循环和联系人暂存目录：先恢复本地缓存，再下载本轮未恢复媒体。默认以及仅设置 `WECHAT_SNS_DOWNLOAD_MEDIA=1` 时均不联网。实际下载扩展名用于单帖、汇总、HTML 和报告；缓存失败但下载成功按最终成功计数，保留原诊断。下载失败不丢弃帖子或其他成功媒体，但 CLI 返回非零退出码。原离线 API 仍不联网，已有 SNS 目录仍拒绝覆盖。

相册图片基础模块已注册，HTML 实体解码复用 SNS 现有实现。通用 WASM 密钥流上限从视频专用前缀中分离，支持至 25 MiB；视频仍只处理前 128 KiB。预算按大小收紧，最高 300M fuel，视频仍为 100M，并遵守调用者更低的上限。Node 合成对照覆盖大图边界，较低预算失败不返回部分结果，随后的小请求仍可成功。该基础模块尚未接入完整相册入口。

回归发现并修复两类问题：测试用非阻塞监听器接受连接后，在 Windows 显式恢复阻塞读取，增加延迟和分段请求头测试；后台陈旧记录处理先核验进程是否存活，再查询身份，避免其他句柄保留已退出进程对象时映像查询失败阻断清理或重启。保留创建时间篡改必须拒绝的断言，并持有旧进程句柄确定性覆盖停止清理和重新启动，两条路径连续五次通过。聊天批量导出继续在账号加载前验证日期与计划参数。

最终 MSVC `cargo check` 和完整 `cargo test --target x86_64-pc-windows-msvc -- --test-threads=1` 均退出 0：18 个测试目标合计 1061 次通过、0 失败、11 忽略，其中主程序为 698/0/7；跨目标包含共享生产模块测试，不代表 1061 个独立功能。编译仍有待接线接口等未使用警告，未隐藏。完整本机日志为 `wx-cli-sns-lifecycle-final-check.log`、`wx-cli-sns-lifecycle-final-tests.log`，失败诊断日志另行保留。旧 Python 实现本轮未修改、未重跑全集；真实账号联网、真实 ASR 模型和 GPU 不在本轮验收范围。

原稳定安装未替换。旧时间线已有目录更新、完整相册、导出/全量工作流及 GUI 等兼容编排、最终模块合并和文件清理仍未完成，不能据本轮回归通过宣称整体迁移完成。
