# Toolkit 数据库语音适配边界优化报告

> 阶段记录：本文保留实施当时的路径、限制和测试结果，不是当前接口规范。当前职责与入口以[架构说明](architecture.md)和[文档索引](README.md)为准。

日期：2026-09-16

## 当前基线与检查范围

本轮以当前脏工作区为基线，只处理 `src/toolkit/asr/database_media.rs` 及其直接引用，不覆盖并行任务正在修改的密钥、发布和任务调度实现。支持目标为 Windows x64 MSVC；构建使用本 checkout 的独立 `CARGO_TARGET_DIR`。

检查范围包括生产调用、两个独立 MCP 夹具、适配器合成 SQLite 测试、模块文档和迁移清单。未访问真实微信账号、真实数据库、真实密钥或外部转录服务。

## 已发现问题

`src/toolkit/asr/database_media.rs` 只有对 `adapters::wechat::media::voice` 的公开重导出，但 daemon、ASR 编排和聊天媒体编排仍依赖该旧路径。两个测试夹具还通过 `#[path]` 再次编译同一适配器源码，形成类型重复和错误的模块所有权。数据库语音契约及测试也仍物理放在 Toolkit。

## 实际修改

- 删除无逻辑的 `toolkit::asr::database_media` 模块。
- daemon、ASR 和聊天导出调用者直接依赖 `adapters::wechat::media::voice` 的现有类型和函数。
- 将 `database_media_tests.rs` 迁为适配器旁的 `voice_tests.rs`，保留全部测试内容。
- 两个 MCP 夹具改为复用其已装配的真实适配器模块，不再重复包含源码。
- 将数据库语音契约迁为 `src/adapters/wechat/media/VOICE_DATABASE.md`，同步全部第一方链接和迁移清单。

没有修改语音关联算法、SQLite 查询、账号边界、错误类型、响应投影、CLI 参数、MCP wire format 或授权顺序。

## 边界与复杂度变化

调用链由“执行/编排 → Toolkit 转发 → 微信语音适配器”缩短为“执行/编排 → 微信语音适配器”。

可验证指标：

| 指标 | 修改前 | 修改后 |
| --- | ---: | ---: |
| 无逻辑生产转发模块 | 1 | 0 |
| 调用适配器所需模块跳数 | 2 | 1 |
| 夹具重复编译适配器源码 | 2 | 0 |
| `toolkit::asr::database_media` 源码引用 | 多处 | 0 |

本轮没有拆分算法函数，因此不声称圈复杂度下降；改进的是依赖方向和重复装配。

## 验证结果

- `cargo check --offline --locked --target x86_64-pc-windows-msvc --all-targets`：通过，无告警。
- `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx adapters::wechat::media::voice::tests -- --test-threads=1`：29 通过，0 失败，0 忽略。
- `cargo test --offline --locked --target x86_64-pc-windows-msvc --test mcp_audio_adapter -- --test-threads=1`：186 通过，0 失败，0 忽略。
- `cargo test --offline --locked --target x86_64-pc-windows-msvc --manifest-path tests/fixtures/mcp-voice-security/Cargo.toml -- --test-threads=1`：104 通过，0 失败，0 忽略。
- 旧路径搜索：生产、测试中均无 `toolkit::asr::database_media`、`asr::database_media` 或旧文件路径残留。

## 保留问题与未验证范围

本段记录该切片完成时的状态：当时仅修复数据库语音适配器所有权。后续切片已将 ASR 应用工作流迁入 `src/application/transcription`、具体后端迁入 `src/infrastructure/transcription`，并删除整个 `src/toolkit`；当前状态以 [转录分层与 Toolkit 删除](architecture-optimization-transcription-2026-09-16.md) 为准。真实微信版本覆盖、真实 SILK 音频质量、本地模型执行和云端上传仍未验证。

本轮没有移除公开命令或磁盘格式，没有需要用户重新初始化的破坏性变化。

## 同轮后续：HTTP 入口归位

确认 `src/toolkit/web` 只承担 Axum HTTP/SSE、协议转换、固定账号 service client 和静态资源后，目录整体迁至 `src/web`。CLI 仍通过同一个 `serve` 实现启动，HTTP 查询与任务操作仍经 service/daemon，静态资源及第三方许可完整保留。架构守卫同步扫描新目录。本切片当时保留的 `wx toolkit run web` 兼容命令已在后续无历史包袱收口中移除，当前正式命令为 `wx toolkit web`。

该迁移将 10 个 Web 文件移出 Toolkit。完成数据库语音和 Web 两个切片时，`src/toolkit` 文件数从 90 降至 77；仓库外部生产源码中引用 Toolkit 的文件数从 44 降至 43。引用文件数只用于跟踪目录消除进度，不代表剩余文件都只有单一依赖。

Web 迁移验证：

- `cargo test --offline --locked --target x86_64-pc-windows-msvc --bin wx web:: -- --test-threads=1`：14 通过，0 失败，0 忽略。
- `cargo test --offline --locked --target x86_64-pc-windows-msvc --test entry_architecture -- --test-threads=1`：9 通过，0 失败，0 忽略。
- Web 迁移后的 all-target check 与严格 all-target Clippy `-D warnings`：通过，无告警。
- 生产和测试源码中 `toolkit::web` 及旧架构扫描路径：0 处。

## 同轮后续：表情工作流归位

微信表情字节格式此前已迁至 `adapters/wechat/emoticons`。本轮将剩余下载、受监督转换和受保护发布工作流从 `src/toolkit/emoticons` 整体迁至 `src/application/emoticons`，daemon 直接调用应用用例。独立下载 fixture 同步引用新所有者；公开命令、下载限制、转换回退和发布语义未改变。

迁移后 `src/toolkit` 文件数进一步降至 74。外部生产源码引用 Toolkit 的文件数回到 44，因为新的应用工作流仍复用 Toolkit 当前公开的 `ExportTarget`；这是文件发布基础设施尚未迁出的明确剩余依赖，不以复制实现或扩大可见性规避。

表情工作流验证：

- `tests/emoticons_runtime.rs`：7 通过，0 失败，0 忽略。
- `tests/entry_architecture.rs`：9 通过，0 失败，0 忽略。
- `tests/fixtures/emoticons-download`：库测试 142 通过、2 忽略；独立监督测试 3 通过。忽略项分别是需要宿主安装带 libx265 的 FFmpeg，以及只由生命周期测试启动的 helper 进程。
- fixture 的 Windows 回环服务器原先让已接受 socket 继承非阻塞模式，可能在请求到达前报 `WSAEWOULDBLOCK`。现于 `accept` 后恢复阻塞模式并保留读取超时；监督测试连续三轮 9/9 通过。
- 根工程和 fixture 的严格 all-target Clippy `-D warnings` 均通过。

## 同轮后续：单文件发布基础设施归位

`src/toolkit/files.rs` 的路径隔离、有限遍历、目标身份固定、同卷暂存和原子提交是通用基础设施。实现整体迁至 `src/infrastructure/publication.rs`，所有生产调用者和 fixture 直接依赖新所有者；Toolkit 未保留转发或重导出。原先只对 Toolkit 父模块可见的 `resolved`、`collect`、`decryption_protected` 仅提升为 crate 内可见，不成为外部 API。

迁移后 `src/toolkit` 文件数为 73，仓库外部生产源码中引用 Toolkit 的文件数从 44 降至 36。架构守卫随后发现 publication 仍反向调用 `toolkit::setup` 的路径检查；通用路径身份与父目录守卫已下沉到 publication，自此 `src/infrastructure` 不再依赖 Toolkit。

发布基础设施验证：

- `infrastructure::publication::export_tests`：13 通过，0 失败，0 忽略。
- `native_migration_security`：4 通过，0 失败，0 忽略。
- `delta_plan_security`：7 通过，0 失败，0 忽略。
- `scripts/check-fixtures.ps1 -Offline`：27/27 fixture 编译通过；所有 fixture 日志无 `warning:`。
- 根工程 all-target check、格式检查和严格 all-target Clippy `-D warnings`：通过。
- 旧 `src/toolkit/files.rs` 路径、Toolkit files/ExportTarget/发布重导出引用：0 处。

fixture 矩阵还发现两个 MCP 语音宿主测试仍从已删除的 `toolkit_asr::database_media` 导入类型；现已改为直接使用 `adapters::wechat::media::voice`，随后完整矩阵通过。

## 同轮后续：配置事务基础设施归位

`src/toolkit/setup.rs` 实际负责配置文件快照、独占锁、陈旧写入检测、账号目录绑定、输出目标校验和原子 JSON 发布，不是 CLI 工具工作流。实现整体迁至 `src/infrastructure/configuration.rs`；初始化、密钥存储、查询代际、MCP 固定账号检查和参数校验均直接使用新所有者。Toolkit 删除模块声明，fixture 也不再伪造 `toolkit::setup` 路径。

通用路径解析、文件身份、父目录固定与目标保护留在 `infrastructure::publication`；配置模块只组合这些机制，不复制发布实现。迁移没有改变当前唯一密钥存储、旧明文配置明确拒绝、账号切换拒绝、环境变量名校验或配置格式。

本切片完成后 `src/toolkit` 文件数从 73 降至 72，仓库外部生产源码中引用 Toolkit 的文件数从 36 降至 31；`src/infrastructure` 对 Toolkit 的源码引用为 0。

配置事务迁移验证：

- `infrastructure::configuration::tests`：8 通过，0 失败，0 忽略，覆盖旧明文配置拒绝、账号切换拒绝、陈旧快照、并发文件身份和原子发布。
- `tests/entry_architecture.rs`：9 通过，0 失败，0 忽略；前端入口禁止直接依赖配置事务，基础设施禁止反向依赖 Toolkit。
- 根工程 `cargo check --all-targets`：通过，无默认告警。
- `scripts/check-fixtures.ps1 -Offline`：27/27 fixture 编译通过；矩阵发现的两个失效 configuration re-export 已删除，三个受影响 fixture 单独复查无告警。

仍未完成的是 ASR、音频、聊天/SNS 导出、图片批处理、清理和运行状态等真实生产职责；因此本报告不声称 `src/toolkit` 已可立即删除。

## 同轮后续：附件引用应用用例归位

`src/toolkit/attachment_refs.rs` 混合了两层职责：微信私有 XML、元数据和缓存布局已经由 `adapters::wechat::media::attachment_content` 实现；本文件自身只剩账号根下的有界枚举、目录与文件句柄固定、候选核验、累计 MD5 预算和只读引用选择。该用例迁至 `src/application/attachment_references.rs`，daemon 查询和聊天媒体导出直接调用应用模块。

迁移同时删除了应用层对 `AttachmentMetadata`、`MessageInput`、解析函数和适配器错误类型的重导出。调用者直接依赖微信适配器解析契约，应用模块只拥有 `Binding`、`FileReference` 和 `find_reference`。协议 JSON 仍由 daemon 投影，字段、错误码、脱敏和账号根来源不变。

本切片完成后 `src/toolkit` 文件数从 72 降至 71，仓库外部生产源码中引用 Toolkit 的文件数从 31 降至 30。验证结果：

- `tests/fixtures/attachment-refs`：87 通过，0 失败，0 忽略。
- `tests/fixtures/native-attachment-security`：61 通过，0 失败，0 忽略。
- 根工程 `cargo check --all-targets`：通过，无默认告警。
- 旧 `toolkit::attachment_refs`、旧源码路径和解析重导出调用：生产与测试源码均为 0。

## 同轮后续：发布上下文归位与命名澄清

`src/toolkit/export_context.rs` 负责在图片、单音频和离线 SNS 视频发布期间固定账号配置或明确观察“配置缺失”，并在最终提交前复核 `ConfigPin`、未来配置路径和保护清单。该职责迁至 `src/application/publication_context.rs`，类型改名为 `PublicationContext`，避免与消息导出适配器的纯投影 `ExportContext` 混淆。

图片工作流、`VoiceToMp3` 和 SNS 视频操作直接使用应用上下文；Toolkit 删除模块声明且不保留类型别名。配置损坏不回退、预期 runtime 不匹配拒绝、缺失路径必须持续缺失以及最终提交复核语义均未改变。

本切片完成后 `src/toolkit` 文件数从 71 降至 70，仓库外部生产源码中引用 Toolkit 的文件数从 30 降至 29。验证结果：

- `application::publication_context::tests`：3 通过，0 失败，0 忽略。
- `tests/native_migration_security.rs`：4 通过，0 失败，0 忽略。
- 根工程 `cargo check --all-targets`：通过，无默认告警。
- 旧 `toolkit::export_context`、旧源码路径及旧应用类型名：生产源码为 0。

## 同轮后续：运行状态诊断归位

`src/toolkit/run_status.rs` 只读取选定配置、目录元信息、导出文件大小和转录进度，不读取密钥正文，也不执行 Toolkit 算法。实现与测试迁至 `src/application/run_status.rs` 和 `run_status_tests.rs`，daemon 的 `Operation::RunStatus` 直接调用应用用例；CLI 命令、短别名、JSON 字段和文本提示不变。

本切片完成后 `src/toolkit` 文件数从 70 降至 68。仓库外部生产源码中引用 Toolkit 的文件数保持 29，因为同一个 daemon 操作模块仍调用其他尚未迁出的 Toolkit 能力；该指标按文件而非符号计数。

验证结果：

- `application::run_status::tests`：2 通过，0 失败，0 忽略。
- `tests/run_status_runtime.rs`：3 通过，0 失败，0 忽略。
- 旧 `toolkit::run_status` 和旧源码路径：生产与测试源码均为 0。

## 同轮后续：清理用例与文件句柄基础设施拆分

原 `src/toolkit/cleanup.rs` 同时包含固定账号清理计划、逐文件授权、执行报告，以及 `cleanup/handles.rs` 中的 Win32 文件身份和按句柄删除。现将计划、授权和执行顺序迁至 `src/application/cleanup.rs`，将路径逐层固定、运行目录锁、文件指纹、硬链接拒绝和句柄删除迁至 `src/infrastructure/cleanup_files.rs`。daemon 直接调用应用用例，Toolkit 旧目录已删除。

基础设施只向 crate 内暴露应用用例确实使用的窄接口；`MAX_FILE_BYTES` 等内部实现仍保持模块私有。密钥删除仍要求本次显式授权、完整文件 ID 选择和精确账号确认，未知文件与目录仍保留。

本切片完成后 `src/toolkit` 文件数从 68 降至 66，仓库外部生产源码中引用 Toolkit 的文件数从 29 降至 28。验证结果：

- `infrastructure::cleanup_files::tests`：4 通过，0 失败，0 忽略。
- `application::cleanup::tests`：9 通过，0 失败，0 忽略。
- 根工程 `cargo check --all-targets`：通过，无默认告警。
- 旧 `toolkit::cleanup`、旧源码目录和 daemon 旧引用：生产与测试源码均为 0。

## 本轮集中验收

验收基线为脏工作区 `main@d4e22a0`，远端为 `https://github.com/leyan2174/wx-workbench.git`。本轮没有提交、推送或覆盖其他任务已有修改；构建始终使用本 checkout 独立的 `C:\CodexLocal\wx-workbench-target-20260916`。

结构指标以本轮首次统计和当前源码为准：

| 指标 | 本轮起点 | 当前 | 说明 |
| --- | ---: | ---: | --- |
| `src/toolkit` 文件数 | 90 | 58 | Web、表情、发布、配置、附件引用、发布上下文、运行状态、清理、数据库解密、图片发布及聊天归档职责已迁出 |
| Toolkit 外生产源码依赖文件数 | 44 | 23 | 按文件计数，同一文件内多个引用只计一次 |
| `src/infrastructure` 对 Toolkit 的源码引用 | 1 | 0 | publication 路径守卫已下沉，基础设施依赖方向闭合 |
| 附件引用应用层适配器重导出 | 1 组 | 0 | XML/元数据类型由调用者直接依赖微信适配器 |

这些是结构复杂度指标。本轮没有机械拆分核心算法，因此不声称所有函数圈复杂度下降；保留的 ASR、聊天与 SNS 热点仍需按业务切片处理。

最终检查：

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`：通过，0 告警。
- `scripts/check-fixtures.ps1 -Offline`：27/27 fixture 编译通过；完整日志 `warning:` 计数为 0。
- `cargo test --target x86_64-pc-windows-msvc`：2952 通过，0 失败，23 忽略。忽略项保留原有环境依赖或 helper-process 语义，没有扩大 ignore。
- `git diff --check`：退出码 0；仅 Git 报告现有工作区 LF/CRLF 转换提示。

## 移除的内部兼容机制

- 删除 `toolkit::setup` 所有权和 fixture 伪造路径；唯一替代为 `infrastructure::configuration`。
- 删除 `toolkit::attachment_refs` 及其适配器类型/解析函数重导出；唯一替代为 `application::attachment_references` 加 `adapters::wechat::media::attachment_content`。
- 删除 `toolkit::export_context::ExportContext` 和类型别名；唯一替代为 `application::publication_context::PublicationContext`。
- 删除 `toolkit::run_status`；唯一替代为 `application::run_status`。
- 删除 `toolkit::cleanup` 及其 handles 子目录；唯一替代为 `application::cleanup` 与 `infrastructure::cleanup_files`。
- 删除 `toolkit::decrypt`、`DecryptMode` 及数据库解密源码；唯一替代为 `application::database_decryption`。
- 删除 Toolkit 图片发布和密钥解析转发；唯一替代为 `application::image_publication`。

这些都是 crate 内部路径变更，不改变当前 CLI、HTTP、MCP wire format、配置磁盘格式或用户数据。用户无需重新初始化；旧明文密钥配置仍按当前唯一密钥政策明确拒绝，不新增回退。

## 当前未完成范围

`src/toolkit` 仍有 44 个文件，集中在 ASR 和 SNS 工作流。它们包含真实算法、外部进程、网络授权或多文件发布语义，不能仅为删除目录整体改名。下一阶段应继续逐域完成“目标层实现、全部调用方切换、旧生产源码删除、行为测试”四件事；在这些职责迁完前，不能声称 Toolkit 已完整删除。

## 数据库解密与图片发布工作流归位

数据库主文件快照工作流已从 `src/toolkit/databases.rs` 迁至 `src/application/database_decryption.rs`。daemon 前台 Operation、准备后全量导出和持久任务 worker 均直接调用应用模块；数据库页面认证继续复用 `crypto`，路径隔离、句柄固定与原子提交继续复用 `infrastructure::publication`。`Strict` 与 `Legacy` 的缺钥策略、批次 JSON 字段、逐项错误和退出语义未改变。

图片工作流已从 `src/toolkit/images.rs` 迁至 `src/application/image_publication.rs`。单图、镜像批次、相册批次和任务 worker 直接调用应用模块；Web、SNS 与 service 参数校验调用同一 AES/XOR 解析器。微信图片格式与解码规则仍由 `adapters::wechat::media::image_batch` 和 attachment decoder 实现。原共享批次结果成为私有的 `application::publication_report`，保持 wire 字段和 `ipc::outcome` 结算，不扩张为通用业务对象。

本切片验证：Windows MSVC all-target check 通过且无编译告警；图片发布 16 项、数据库解密 1 项、批次结果 1 项、架构守卫 9 项全部通过；`mcp-image-security` fixture 20 项通过，2 项因当前 Windows 账户无符号链接权限按既有条件忽略。旧 Toolkit 解密/图片入口及旧源码路径引用为 0。

## 聊天归档文档合并归位

原 `src/toolkit/chat_merge.rs` 直接读取和生成稳定的聊天导出 JSON 文档。它虽然没有 IO，但其输入输出是协议投影而不是普通 Message 业务对象，因此迁至 `src/application/chat_archive_merge.rs`，没有并入 `business/archive`。业务归档编排继续通过泛型把文档视为不透明值，避免业务层依赖 `serde_json::Value` 或旧导出字段。

`export_chats` 已直接调用应用模块，Toolkit 不保留转发。既有 username 一致性、`source + local_id` 去重、未知来源歧义原子失败、整数极值排序、旧顶层字段优先和输入不修改语义均保留。模块 5 项行为测试及 9 项入口架构守卫通过；根工程 Windows MSVC all-target check 通过。迁移后 Toolkit 文件数从 64 降至 62；外部依赖文件数保持 25，因为同一调用方仍使用尚未迁出的聊天索引能力。

## 计划选择与聊天归档索引归位

人工计划 CSV 读取从 `src/toolkit/chat_plan_selection.rs` 迁至 `src/application/chat_plan_selection.rs`。`Mode` 仍由 service 请求契约拥有，应用模块和 daemon 直接使用该枚举，不再通过 Toolkit 反向重导出。16 MiB/100000 行上限、严格引号边界、username 唯一性、白名单/黑名单和未知选中账号整批拒绝语义保持不变。独立 fixture 3 项通过，使用当前 checkout `wx.exe` 的 CLI → daemon → 合成加密 SQLite 运行时测试 1 项通过。

聊天输出索引从 `src/toolkit/chat_index.rs` 迁至 `src/application/chat_archive_index.rs`。它负责账号绑定、同名避碰、索引锁和原子发布，不包含微信数据库或格式规则；`export_chats` 直接依赖新模块。索引模块 7 项、`native_migration_security` 4 项和架构守卫 9 项通过。无索引目录扫描恢复及 `legacy_unverified` 标记仍是明确待审计的兼容行为，本轮没有在文件移动时顺带改变磁盘语义。

这两个切片完成后 Toolkit 文件数从 62 降至 60。外部生产依赖文件数保持 25，因为相同 daemon 文件仍调用尚未迁出的 `chat_delta`、`chat_plan` 等能力；旧 Toolkit 计划选择和聊天索引路径引用均为 0。

## 聊天增量导出应用切片归位

原 `src/toolkit/chat_delta.rs` 已迁至 `src/application/chat_delta_export.rs`。该模块拥有增量窗口、稳定产物 UID、原始内容传输表示、JSON 文档投影和独占运行目录发布；它不读取微信表或数据库。微信分片完整性、消息字段映射、压缩内容读取和联系人元数据仍由 `daemon/query/export_delta.rs` 通过现有微信 adapter 完成，`business/archive` 继续只负责目标级顺序、身份核对和完成语义。

daemon 操作、全量导出的 delta-only 路径、service 参数校验和查询 adapter 均直接使用应用模块，Toolkit 不保留转发。专项 harness 验证 query/operation 11 项、应用发布 15 项、业务归档 4 项；过程级 `delta_plan_security` 7 项和 `delta_runtime` 8 项通过。覆盖未知/消失分片拒绝、查询失败不发布部分聊天、manifest 失败、闭区间、追加批次不覆盖、受保护路径和运行名校验。

迁移后 Toolkit 文件数从 60 降至 58，外部生产依赖文件数从 25 降至 23。旧 Toolkit delta 路径仅曾残留于专项 Python 测试过滤器，现已同步为 `application::chat_delta_export`；生产、Rust 测试与脚本中的旧路径为 0。

## 聊天导出计划应用切片归位

原 `src/toolkit/chat_plan.rs` 已迁至 `src/application/chat_export_plan.rs`。应用模块只协调只读统计、媒体扫描、业务规则和 CSV 投影；`PlanChat`、`SizeMode`、`TimeRange` 与统计完整性语义继续由 `business/chat_plan.rs` 拥有，SQLite 查询、分库排序、资源与媒体缓存布局继续由 `adapters/wechat/planning` 拥有。daemon 的计划命令和全量导出直接依赖这些真实所有者，不经 application 或 Toolkit 反向重导出公共类型。

专项验证包括应用模块 17 项、业务模型 3 项、微信 planning adapter 4 项、`chat_plan_runtime` 7 项和 `delta_plan_security` 7 项，均使用合成数据库与隔离输出。既有 CSV BOM/CRLF、闭区间、部分结果、精确整数、路径保护和失败不覆盖语义保持不变。迁移后 Toolkit 文件数从 58 降至 56，外部生产依赖文件数从 23 降至 20；旧 Toolkit chat plan 源码路径在生产代码、Rust 测试与脚本中为 0。

## 聊天目录导出应用切片归位

原 `src/toolkit/chat_directory.rs`、`chat_directory/media.rs` 和 `render.rs` 已整体迁至 `src/application/chat_directory*`。该用例消费 daemon 查询得到的已解析消息，生成 CSV、HTML、JSON 和媒体清单，并通过 `infrastructure::output_tree` 发布；消息目录、微信媒体关联和格式解析仍由 daemon 查询与 adapters 提供。daemon 导出入口和 service 参数校验均直接使用应用模块，Toolkit 不保留转发。

应用模块 6 项、目录入口筛选 4 项、`native_migration_security` 4 项和 `voice_runtime` 5 项通过；Windows MSVC all-target check 通过。迁移后 Toolkit 文件数从 56 降至 53，外部生产依赖文件数从 20 降至 19。剩余 1 个应用层 Toolkit 依赖是媒体准备直接复用尚待迁移的 ASR 能力，不复制实现；旧 Toolkit chat directory 路径在生产代码、Rust 测试与脚本中为 0。

## 音频基础设施与批次应用归位

原 `src/toolkit/audio` 已按职责拆分：SILK/WAV 校验、PCM 转换、受控 ffmpeg、MP3/WAV 发布归 `src/infrastructure/audio`，数据库语音批次协调归 `src/application/voice_batch_export.rs`。微信数据库遍历继续由 `adapters/wechat/media/voice_export` 提供，业务计数继续由 `business/voice_export` 提供。daemon 前台 operation、持久任务 worker、MCP voice、ASR 和聊天目录直接依赖真实所有者，Toolkit 不保留音频包装。

WAV parser、`prepare_wav_bytes` 和 PCM→WAV 从 ASR 移入 audio，消除了发布基础设施反向依赖 ASR 的关系；`mcp-image-security` fixture 同时删除了通过 build.rs 抽取 parser AST 的重复实现。音频模块 14 项通过、1 项因未显式要求系统 ffmpeg/ffprobe 而忽略；批次应用 16 项通过、1 项同因忽略；ASR pipeline 11 项通过。独立 WAV fixture 的库测试 198 项通过、2 项既有条件忽略，发布契约 1 项通过；符号链接场景因当前 Windows 权限明确标为 `UNVERIFIED`。

迁移后 Toolkit 文件数从 53 降至 44，外部生产依赖文件数从 19 降至 17。旧 Toolkit audio 路径在生产代码、Rust 测试和可执行脚本中为 0；历史审计报告保留当时命令和路径，不作为当前入口说明。

## 朋友圈应用工作流归位

原 `src/toolkit/sns` 已整体迁至 `src/application/moments`。时间线、缓存恢复、显式授权下载、归档和相册仍是应用工作流；微信数据库、XML/DAT、缓存布局和 WxIsaac64 密钥流继续由 `business::moments` 与 `adapters::wechat` 提供，路径固定和发布继续复用 infrastructure。daemon 四个朋友圈操作直接调用应用模块，Toolkit 不保留转发。

时间线导出把整批联系人绑定预检和发布拆成独立阶段：所有输出树在内容生成前完成身份检查并持有锁，避免后续联系人冲突时先发布前面的联系人；单联系人暂存、媒体优先发布顺序和旧目录显式认领语义保持不变。模块测试 85 项通过、2 项条件忽略；timeline 8、album 12、download 4、图片夹具 16、视频夹具 14 项通过。

迁移后 Toolkit 文件数从 44 降至 27，剩余文件全部属于 ASR。旧 `toolkit::sns` 生产、测试和脚本路径为 0；README 当前契约改为指向微信媒体适配器。下一步将 ASR 按转录用例与音频后端基础设施拆分，完成后删除 `src/toolkit`、`src/toolkit/mod.rs` 和根 `mod toolkit`，不保留空目录或兼容 facade。

## ASR 分层与 Toolkit 删除

转录、缓存、回执、回写和数据库批次已归 `src/application/transcription`；whisper.cpp、Python Whisper、OpenAI-compatible 客户端及其 Windows 监督适配已归 `src/infrastructure/transcription`；后端身份与纯参数校验由 `src/service/operation_requests/asr_backend.rs` 拥有。Python 后端不再返回应用层类型，也不再调用应用缓存模块计算程序摘要。

主程序、daemon、service 和独立 fixture 均已切换到真实所有者。`src/toolkit` 文件数从 27 降至 0，目录、`src/toolkit/mod.rs` 和根 `mod toolkit` 已删除；没有留下 facade。`wx toolkit`、`cli::toolkit` 与 `daemon::operations::toolkit` 是当前正式命令和操作分组，不是被删除的源码层。

Windows MSVC all-target check 通过；应用转录 74 项、后端基础设施 34 项、ASR/video security 340 项和 MCP audio 186 项通过。安全集成保留 2 项既有条件忽略（系统 ffmpeg/ffprobe、仅由生命周期测试启动的 helper）。真实微信账号、真实模型质量和真实云上传仍未验证。
