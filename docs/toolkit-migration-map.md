# Toolkit 功能迁移去向表

## 范围与状态

`src/toolkit` 已删除，而不是整体改名；正式 `wx toolkit` 命令分组继续存在。本表记录各职责的最终去向和迁移证据。

- 核对日期：2026-09-16；工作区 HEAD 为 `d4e22a0`，**读取的是包含并行修改的当前源码，不是该提交的静态内容**。
- 参考了仓库外的内部只读架构审查报告。以下分类已结合实际 imports、调用点、实现片段和测试注册复核，报告中的旧行号不作为当前完成证据。
- 当前已建立 `application`、`infrastructure`、`adapters/wechat` 和顶层 `web` 边界；表中测试结果以各迁移切片的实际记录为准。
- 密钥与 worker 边界以当前 daemon 快照、broker 和配置事务实现为准；仍未覆盖的 bootstrap 与部分读取路径在最终报告中单独列出，不从 Toolkit 迁移状态推断完成。

表内较早的“未迁/部分外移”说明用于保留迁移时判断依据；顶层和子域完成态以当前链接及加粗状态为准。

表内路径缩写：`T/` = `src/toolkit/`，`D/` = `src/daemon/operations/`，`F/` = `tests/fixtures/`；`内测`指所列源码实际注册的测试。测试栏是现有回归入口，不是通过记录，也不保证独立 fixture 被根项目测试自动执行。

最终交叉检查：`src/toolkit` 目录、根 `mod toolkit` 和生产/测试/脚本中的 `toolkit::asr`、`toolkit::sns` 路径均不存在。Windows MSVC all-target check、严格 Clippy 和受影响夹具已运行；`F/` 下并非每个目录都是独立测试工程，具体结果见各迁移报告。

## 归属规则

| 目标类别 | 应接收的职责 | 不应接收的职责 |
| --- | --- | --- |
| `application`，按导出、计划、维护、转录、监控拆分 | 完成一次用例的编排、输入快照、授权后的资源使用、结构化结果及进度 | 不创建新的 daemon，不把所有业务、SQL、HTTP 或进程控制装进同一个 service |
| `adapters/wechat` | 微信表结构、消息及媒体格式、缓存布局、微信供应商特有协议 | 不接管用户授权、全局配置发现、后台生命周期或整次导出发布 |
| `infrastructure/files` | 已验证文件句柄、ACL、锁、快照、路径隔离、有限遍历、原子文件发布 | 来源采用政策、账号授权及清理选择规则仍由上层决定；不许夸大成整树事务 |
| `infrastructure/config` | 配置文档读取、受控更新、配置路径及显式后端设置 | 不扫描微信、不采集密钥、不悄悄回退旧凭据；复用共享配置校验 |
| `infrastructure/audio` | SILK/WAV/MP3 编解码、明确选择的 ASR 后端、音频缓存存储 | 不识别微信账号、不查询微信消息库、不决定跨账号复用策略 |
| 顶层 `web` 入口 | HTTP、SSE、请求转换、限流、页面与资源、服务调用 | 不读微信数据库或密钥、不本地扫描媒体、不替代 daemon 执行业务 |
| 已有 `business` | 复用 `archive`、`chat_plan`、`moments`、`messages`、`attachment_content`、`media`、`voice`、`voice_export` 等规则 | 不引入文件系统、SQLite、后端程序名称或旧 JSON 文档作为全局领域模型 |

daemon 保留账号资源、权限、运行生命周期和 worker 接续；service 保留既有 IPC wire 契约及认证。迁移应用类型不等于删除 IPC，也不等于把所有跨进程调用改为直接调用。

## 顶层模块

本节覆盖 `T/mod.rs` 中全部顶层模块。目录模块在后续子域表展开；独立测试文件随所属功能迁移，不当作生产功能删除。

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| 原 `src/toolkit/mod.rs` | 已删除的临时迁移装配入口 | 无 | 各职责直接位于 application、adapters、infrastructure、web 或 service 契约 | all-target check；架构依赖守卫 | **已删除**；根 `mod toolkit`、目录和兼容转发层均不存在；`cli::toolkit` 仍是正式命令分组 |
| [attachment_references.rs](../src/application/attachment_references.rs) | 有界枚举、固定句柄、候选核验、累计 MD5 预算和受控只读引用 | `src/daemon/query/mcp_attachments.rs`；聊天目录媒体导出 | 已归 `application`；微信 XML、元数据与缓存布局只由 `adapters/wechat/media/attachment_content` 提供，不经应用层重导出 | `F/attachment-refs/` 87 项；`F/native-attachment-security/` 61 项 | **已迁移**；Toolkit 不再声明附件引用模块，wire 投影保持在 daemon 查询边界 |
| [transcription](../src/application/transcription/mod.rs)、[backend infrastructure](../src/infrastructure/transcription/mod.rs) | 文件/字节转录、缓存、回执、回写、数据库批次及具体 ASR 引擎 | `D/asr.rs`、`D/asr_database.rs`、`D/asr_batch.rs`；daemon MCP 语音入口 | 用例归 application；进程/HTTP/引擎归 infrastructure；后端请求契约归 service；微信语音归 adapter | application 74、infrastructure 34、ASR/video security 340/2 ignored、MCP audio 186 | **已迁移**；Toolkit 目录已删除，service 不再反向依赖执行模块 |
| [audio/](../src/infrastructure/audio/mod.rs)、[voice_batch_export.rs](../src/application/voice_batch_export.rs) | SILK/WAV 校验、MP3 编码与发布；数据库语音批量导出 | `D/toolkit.rs`、`D/task_worker.rs`、MCP voice；ASR 管线 | 编解码/受控进程/发布已归 infrastructure；批次编排已归 application；微信读取留 adapters | audio 14 项；batch 16 项；ASR pipeline 11 项；条件集成各忽略 1 项 | **已迁移**；Toolkit 无音频模块或转发，WAV parser 不再由 ASR 反向拥有 |
| [chat_delta_export.rs](../src/application/chat_delta_export.rs) | 增量窗口、稳定产物 UID、原始内容表示、delta 文档投影与运行目录发布 | `D/export_delta.rs`、`D/export_all.rs`；查询 adapter；service 请求校验 | 已归 `application`；目标编排复用 `business/archive`，微信分片/字段读取仍在 adapter/query，文件提交复用 infrastructure | 本模块 15 项；query/operation 11 项；business 4 项；runtime 8 项；security 7 项 | **已迁移**；Toolkit 无转发，查询失败、部分结果、manifest 和不覆盖语义不变 |
| [chat_directory.rs](../src/application/chat_directory.rs)、`chat_directory/media.rs`、`render.rs` | 已解析聊天的目录、媒体准备、CSV/HTML/JSON 输出和绑定发布；不自行完成消息查询 | `D/export_messages.rs`；service 参数校验 | 已归 `application`；微信消息/媒体规则继续归 adapters，文件提交归 infrastructure；媒体准备暂时直接复用待迁移 ASR 模块 | 本模块 6 项；入口筛选 4 项；`tests/native_migration_security.rs` 4 项；`tests/voice_runtime.rs` 5 项 | **已迁移**；Toolkit 无转发，磁盘目录、文档字段和部分媒体结果语义不变 |
| [chat_archive_index.rs](../src/application/chat_archive_index.rs) | username 归属、同名避碰、索引读取、账号绑定检查及原子索引维护 | `D/export_chats.rs` | 已归 `application` 归档工作流；锁和原子文件提交复用 infrastructure | 本模块 7 项；`native_migration_security` 4 项；架构守卫 9 项 | **已迁移**；Toolkit 无转发；无索引目录恢复和旧绑定标记仍待无历史包袱审计 |
| [chat_archive_merge.rs](../src/application/chat_archive_merge.rs) | 导出 JSON 增量合并：保留原记录、按来源与 ID 去重、拒绝歧义、稳定排序 | `D/export_chats.rs` | 已归 `application` 归档文档投影；`business/archive` 继续将文档视为不透明泛型 | 本模块 5 项 golden/反例；架构守卫 9 项 | **已迁移**；Toolkit 无转发，JSON 结构未错误提升为业务模型 |
| [chat_export_plan.rs](../src/application/chat_export_plan.rs) | 协调只读数据库统计、媒体扫描、计划规则及 CSV 投影 | `D/chat_plan.rs`、`D/export_all.rs` | 已归 `application`；业务统计和完整性语义由 `business/chat_plan` 拥有，SQLite、分库和缓存布局由 `adapters/wechat/planning` 拥有 | 本模块 17 项；business 3 项；adapter 4 项；`tests/chat_plan_runtime.rs` 7 项；`tests/delta_plan_security.rs` 7 项 | **已迁移**；daemon 直接依赖真实所有者，Toolkit 无转发，CSV/wire/退出语义不变 |
| [chat_plan_selection.rs](../src/application/chat_plan_selection.rs) | 有界读取人工 CSV，以 username 选取白名单/黑名单，不信任显示名和统计列 | `D/export_all.rs`、`D/export_chats.rs` 读取 `Plan`；service 拥有 `Mode` | 已归 `application` 计划输入；请求枚举仍由 service 契约拥有，不经应用层重导出 | `F/plan-selection/` 3 项 + 真实 CLI/daemon runtime 1 项 | **已迁移**；Toolkit 无转发，CSV 大小/行数/引号和身份约束不变 |
| [cleanup.rs](../src/application/cleanup.rs)、[cleanup_files.rs](../src/infrastructure/cleanup_files.rs) | 固定账号清理计划、逐文件授权执行、旧产物显式采用；句柄层只删除核验过的同一对象 | `D/cleanup_native.rs` | 计划、授权和报告已归 `application`；Win32 文件身份、锁、指纹及按句柄删除归 `infrastructure` | application 9 项；infrastructure 4 项，覆盖变更文件、硬链接、陈旧指纹、密钥再授权与运行锁 | **已迁移**；Toolkit 不保留模块，元数据统计与可执行删除仍明确区分 |
| [database_decryption.rs](../src/application/database_decryption.rs) | 使用已提供密钥导出数据库主文件快照，验证首页、固定源和配置、原子写入；不合并实时 WAL | `D/toolkit.rs` 的 `Decrypt`；`D/task_worker.rs`；正式全量导出 | 已归 `application`；继续复用既有 `crypto`、基础设施 SQLite 验证器和 `infrastructure::publication` | 本模块密钥格式测试；`tests/run_decrypt_runtime.rs`；根工程 all-target check | **已迁移**；只保留严格缺钥/逐项失败语义，Toolkit 无转发入口 |
| [output_tree/](../src/infrastructure/output_tree/mod.rs) | 共享输出树预检、来源绑定、既有目录采用政策、逐文件发布；明确不是整树事务 | 聊天目录、SNS 导出/归档/相册及 `D/sns_album.rs` | 已移至文件系统基础设施层；绑定与 adopt/update 仍由调用用例显式传入 | `infrastructure/output_tree/tests.rs`；`F/sns-publish/`；SNS runtime 测试 | 已迁；无 Toolkit 转发层，保留锁文件名与按文件提交语义 |
| [emoticons/](../src/application/emoticons/mod.rs) | 根模块装配 `download`；工作流负责 URL 媒体获取、受监督转换及输出保护 | `D/export_emoticons.rs` → `application/emoticons/download.rs` → `adapters/wechat/emoticons/remote_format.rs` | 表情导出工作流已归 application；微信 AES/填充/格式标记和流定位归适配器，文件/进程机制复用既有基础设施 | `remote_format_tests.rs`；`application/emoticons/download_tests.rs`；`F/emoticons-download/`；`tests/emoticons_runtime.rs` | **已迁移**；Toolkit 不再拥有表情模块 |
| [publication_context.rs](../src/application/publication_context.rs) | 固定账号或显式离线上下文，区分缺配置与已存在配置，发布前复核 ConfigPin 和保护路径 | `images.rs`；`D/toolkit.rs`、`D/sns_video.rs` | 已归 `application`；复用既有 runtime/config pin，不把账号策略放入通用 publication | 本文件 3 项安全测试；`tests/native_migration_security.rs` 4 项 | **已迁移**；类型改名为 `PublicationContext`，Toolkit 不保留旧模块或类型别名 |
| [publication.rs](../src/infrastructure/publication.rs) | 路径解析与隔离、有限遍历、同卷原子输出；`ExportTarget` 接收调用方提供的账号保护路径 | daemon、application、图片、音频与 SNS 发布直接依赖基础设施 | 已整体归 `infrastructure::publication`；账号保护集合仍由固定 RuntimeContext 生成，调用方保留授权责任 | 本模块 13 项原子发布测试；`tests/native_migration_security.rs`、`tests/delta_plan_security.rs`；27 项 fixture 编译矩阵 | **已迁移**；Toolkit 不再声明或重导出 files，现有安全实现未复制 |
| [image_publication.rs](../src/application/image_publication.rs) | 固定图片输入/配置、材料解析、批次布局选择、保护发布；格式识别和解码规则委托适配器 | `D/toolkit.rs`；`D/task_worker.rs`；SNS/Web 参数校验 | 已归 `application`；微信格式规则仍归 `adapters/wechat/media/image_batch`，发布归 `infrastructure::publication` | 本模块 16 项；`F/mcp-image-security/` 20 通过、2 权限忽略；架构守卫 9 项 | **已迁移**；密钥解析和三个发布入口均无 Toolkit 转发，命令参数与输出字段不变 |
| `legacy.rs`（已删除） | 原先用于定位仓库内兼容 Python 工具链 | 无 | Python 解释器发现收敛到 ASR 后端，状态诊断只报告原生能力 | ASR 本地后端测试；全仓库引用扫描 | 已删除；不再支持 `WX_WECHAT_DECRYPT_DIR`/`WX_WECHAT_DECRYPT_PYTHON` |
| [application/monitor](../src/application/monitor/mod.rs) | 固定账号轮询、游标、终端渲染和延迟观测；不负责启停后台 | `D/monitor_native.rs`、Web 查询监控 | 已迁至 `application`；取消令牌与控制台处理在 `infrastructure::cancellation`，延迟回包在 `service::protocol::monitor` | `latency.rs`、`statistics.rs`、`chunked.rs` 内测；架构依赖测试 | 已迁；无 Toolkit 转发层，客户端观测语义仍不是可靠消息队列 |
| [private_file.rs](../src/private_file.rs) | 对已打开文件设置私有权限，Windows ACL 检查辅助 | `src/service/transport.rs`；配置/密钥文件路径、任务历史、音频暂存和图片缓存 | 已迁至独立私有文件基础设施；复用句柄 ACL，不转为先写后改权限 | `src/service/transport.rs` ACL 内测；`src/key_store/tests.rs`；独立夹具 | 已迁；旧模块和转发入口已移除，权限实现保留 |
| [run_status.rs](../src/application/run_status.rs) | 按选定配置统计目录、导出进度和密钥文件元信息；不读取密钥正文 | `D/mod.rs` 的 `Operation::RunStatus` | 已归 `application` 只读维护诊断；有限数据库遍历复用 publication | 本模块 2 项测试；`tests/run_status_runtime.rs` 3 项 | **已迁移**；Toolkit 不保留模块或转发，CLI 命令及 JSON/文本输出保持不变 |
| [configuration.rs](../src/infrastructure/configuration.rs) | 配置快照/锁、目标校验、安全配置文档更新和初始化公共读写 | `D/init.rs`、`D/setup_native.rs`、查询代际、MCP 固定账号检查及 `src/key_store/mod.rs` | 已整体归 `infrastructure::configuration`；通用路径身份与原子发布复用 `infrastructure::publication` | 本文件 8 项行为测试；`src/key_store/tests.rs`；`tests/current_entry_contract.rs`；27 项 fixture 编译矩阵 | **已迁移**；Toolkit 不再声明 setup，生产和 fixture 均不保留旧路径转发 |
| [moments/](../src/application/moments/mod.rs) | SNS 导出、缓存恢复、归档、相册和受控媒体获取工作流 | `D/export_sns.rs`、`D/sns_timeline.rs`、`D/sns_archive.rs`、`D/sns_album.rs`；单视频操作直接使用微信媒体适配器 | 已归 `application`；微信格式、缓存布局和密钥流仍归 adapters，发布归 infrastructure | 模块 85 通过/2 条件忽略；timeline 8、album 12、download 4、图片夹具 16、视频夹具 14 | **已迁移**；Toolkit 不保留 SNS 模块或转发，授权与 wire/CLI 语义不变 |
| [web/](../src/web/mod.rs) | Axum HTTP/SSE、固定账号服务访问、页面资源，不是业务引擎 | `src/cli/web_native.rs` → `web::serve` | 顶层 `web` 入口；继续通过 service 访问 daemon | `src/web/mod.rs` 内测；`src/web/automatic_image_runtime_tests.rs` | **已迁移**；HTTP 入口与资源已整体移出 Toolkit，没有移入 business 或 daemon 用例模块 |

## ASR 子域

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| [transcription/mod.rs](../src/application/transcription/mod.rs) | 后端组合、字节/文件转录管线和离线媒体清单 | `D/asr.rs`、`D/asr_database.rs`；批次及 MCP 语音调用 | 应用用例；WAV/SILK 复用 audio infrastructure | 应用模块 74 项中的 pipeline/byte tests | **已迁移**；不重导出具体引擎模块 |
| [asr_backend.rs](../src/service/operation_requests/asr_backend.rs) | 后端身份、入口限制和纯配置校验 | service 请求、settings/plan、daemon 装配 | service 公共请求契约 | 契约测试被根工程和安全夹具执行 | **已迁移**；service 对 Toolkit/应用执行模块反向依赖为 0 |
| [batch.rs](../src/application/transcription/batch.rs)、[batch/files.rs](../src/application/transcription/batch/files.rs) | 账号数据库快照批量转录、逐条缓存和最终聊天 JSON 发布 | `D/asr_batch.rs`；`D/export_messages.rs` | 应用批次用例；daemon 继续提供账号 DbCache，发布复用 infrastructure | 应用模块批次与身份反例；ASR/video security | **已迁移**；快照加载失败不推进回写 |
| [cache.rs](../src/application/transcription/cache.rs)、[cached.rs](../src/application/transcription/cached.rs)、[receipt.rs](../src/application/transcription/receipt.rs) | 账号/音频/配置隔离缓存、成功提交和强证据回执协调 | database ASR、batch、MCP voice | 应用状态与提交协调；文件摘要由 infrastructure 提供 | cache/cached/receipt 测试均在 74 项应用测试内 | **已迁移**；缓存故障、授权拒绝和证据歧义语义保持 |
| [voice.rs](../src/adapters/wechat/media/voice.rs) | 账号绑定的微信语音数据库关联与证据验证 | `D/asr_database.rs`、`D/export_messages.rs`、`src/daemon/query/mcp_audio.rs`、batch 直接依赖适配器 | 已归 `adapters/wechat/media`；`toolkit/asr/database_media.rs` 已删除 | `voice_tests.rs` 与 MCP 音频、安全夹具 | **已迁移**；生产与夹具均不再经过 Toolkit 转发层 |
| [local.rs](../src/infrastructure/transcription/local.rs)、[local_python.rs](../src/infrastructure/transcription/local_python.rs) | whisper.cpp 与 Python Whisper 的受监督执行、资源限制和引擎身份 | application Backend；daemon 装配 | 转录基础设施，复用唯一 Windows managed process | infrastructure 34 项；ASR/video security 后代回收测试 | **已迁移**；基础设施返回自身结果，不依赖应用 Transcription 或缓存模块 |
| [openai.rs](../src/infrastructure/transcription/openai.rs) | 显式 OpenAI-compatible multipart 客户端、认证和响应限制 | application Backend；daemon 装配 | 转录网络基础设施；授权仍由宿主/应用检查 | 回环 HTTP 测试包含在 infrastructure 34 项和安全集成 | **已迁移**；不读取默认凭据，不隐式云回退 |
| [prepared_audio.rs](../src/application/transcription/prepared_audio.rs) | 有界内存音频打包与证据校验 | daemon query/server、MCP voice | 应用准备用例；IPC 预算仍由宿主转换 | 应用 7 项；MCP audio 186 项 | **已迁移**；准备成功不等于业务转录成功 |
| [writeback.rs](../src/application/transcription/writeback.rs) | 按 username/source/local_id 定位聊天 JSON 语音并原子发布 | 离线清单与 batch | 应用回写用例 | 应用 writeback 14 项 | **已迁移**；不写原微信库、不猜分片 |
| [windows_supervision.rs](../src/infrastructure/transcription/windows_supervision.rs) | 为 Whisper/Python 增加调用者上下文并复用 managed process | local/local_python | 转录基础设施内部辅助 | local 与 managed 生命周期测试 | **已迁移**；没有第二套 Job Object 引擎 |

后端契约见 [asr-backends.md](asr-backends.md)，缓存/回写说明随 application，具体本地/云端后端说明随 infrastructure。数据库语音契约与测试位于 `adapters/wechat/media/VOICE_DATABASE.md` 和 `voice_tests.rs`。

## 音频子域

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| [audio/mod.rs](../src/infrastructure/audio/mod.rs) | SILK 规范化/SDK 解码、ffmpeg MP3 转换及限制 | ASR 字节管线；`D/toolkit.rs` VoiceToMp3；批次应用 | 已归 audio infrastructure；共享进程控制不复制 | `audio/tests.rs`、`process_tests.rs`；`F/audio/` | **已迁移**；生产路径不依赖 Python oracle |
| [voice_batch_export.rs](../src/application/voice_batch_export.rs) | 配置/联系人筛选、解密 media DB 批次协调、输出目录及计数 | `D/toolkit.rs` VoiceBatch；`D/task_worker.rs` | 已归 application；微信读取复用 adapters，转换归 audio infrastructure | 本模块 16 项；`tests/voice_runtime.rs`；`tests/native_migration_security.rs` | **已迁移**；Toolkit 无包装，报告 wire 与逐项发布语义不变 |
| [audio/publish.rs](../src/infrastructure/audio/publish.rs)、`wav.rs` | 校验 WAV 后本地无覆盖发布，宿主回调检查身份、取消和响应预算 | `src/daemon/mcp_service/voice.rs`；`F/wav-publish/` | 已归 audio infrastructure；publication 负责固定输出/提交；宿主保留授权回调 | `F/wav-publish/`；MCP WAV 发布内测；`tests/mcp_audio_adapter.rs` | **已迁移**；WAV parser 由 audio 拥有，ASR 仅消费，不再反向依赖 |
| `audio/process_fixture.rs`、`process_tests.rs`、`tests.rs`、`batch_tests.rs` | 进程与编解码合成测试，不是生产服务 | `infrastructure/audio/mod.rs` 与应用批次测试注册链 | 已随 audio infrastructure/批次用例迁移并保留测试归属 | 当前这些测试本身；`README.md`、`BATCH.md` 为契约说明 | **已迁移**；继续保留超时、输出限制和子孙进程回收断言 |

## 监控子域

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| [monitor](../src/application/monitor/mod.rs) | 固定上下文轮询、初始策略、事件和停止原因；秒级游标不是可靠队列 | daemon 前台监控与 Web 监控 | `application` 监控 | 分块/统计与游标恢复测试 | 已迁；真实控制台中断和管道断连仍待验收 |
| [cancellation.rs](../src/infrastructure/cancellation.rs) | 原子取消标记、Windows 控制台回调安装与 RAII 移除 | monitor/latency、`service/operation_client.rs` | 窄基础设施 | 监控编译/行为测试；尚缺真实控制台回调卸载回归 | 已迁；不属于监控业务模型 |
| [service/monitor.rs](../src/service/monitor.rs) | 分块协议限额与 `LatencyProbe` wire 契约/校验 | daemon 生成、application 消费 | 共享 service 契约 | 非法字段、非有限数、未执行阶段不伪造零耗时 | 已从 `daemon::cache` 移出，wire 字段不变 |

## SNS 子域

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| [moments/mod.rs](../src/application/moments/mod.rs) | 子域装配及应用结果投影 | `D/export_sns.rs`、`D/sns_timeline.rs` 等 | 已归应用层；格式类型直接使用既有 adapters/business | 模块测试随实现归位 | **已迁移**；旧 SNS 装配已删除 |
| 原 `sns/decode.rs` | 纯转发文件已删除 | SNS 装配直接导入适配器，供相册与测试使用 | [moments/decode.rs](../src/adapters/wechat/moments/decode.rs) | `sns/tests.rs`；相册图片测试 | 已移除转发；解析算法未重写 |
| 原 `sns/parse.rs` | 纯转发文件已删除 | SNS 装配直接使用现有 Post/Comment/TimeZone 投影 | [moments/legacy.rs](../src/adapters/wechat/moments/legacy.rs) | `sns/tests.rs` | 已移除转发；导出格式投影仍有实际用途 |
| [html_entities.json](../src/adapters/wechat/moments/html_entities.json) | HTML 实体数据 | 相邻 decoder 的 `include_str!` | 已归属微信 moments 适配器 | `sns/tests.rs` 的解码回归 | 已迁，移动前后 SHA-256 一致；不再反向引用 Toolkit 资源 |
| [export.rs](../src/application/moments/export.rs) | 组织联系人/评论、业务时间线查询、作者兼容投影、媒体恢复/下载和绑定发布 | `D/export_sns.rs`、`D/sns_timeline.rs` | 已归 `application`；查询复用 `business/moments::query` 与 adapters，文件机制归 infrastructure | 模块导出测试；`tests/sns_timeline_runtime.rs` 8 项 | **已迁移**；整批绑定预检先于内容生成，发布阶段独立 |
| [cache.rs](../src/application/moments/cache.rs) | 受控源快照与恢复写入，微信 DAT/布局解释委托 adapter | moments export、archive | 恢复编排已归 `application`；格式规则留 `adapters/wechat/moments/cache` | cache 内测及 timeline runtime | **已迁移**；无 Toolkit facade |
| [download.rs](../src/application/moments/download.rs) | 显式宿主授权后的 SNS HTTP 获取、字节/时间/重定向限制和安全输出 | moments export/album | 已归 `application`，输出保护复用 infrastructure | download 内测；`tests/sns_download_runtime.rs` 4 项 | **已迁移**；模型参数不能授予网络权限 |
| [archive.rs](../src/application/moments/archive.rs) | 独立扫描朋友圈缓存图片并归档，完整记录失败/缩略图 | `D/sns_archive.rs` | 已归 `application`，复用微信缓存/解码适配与共享发布 | moments 模块测试及发布回归 | **已迁移**；不以过滤后的索引缩小归档范围 |
| [album.rs](../src/application/moments/album.rs) | 来源绑定相册编排、图片/视频准备和渲染发布 | `D/sns_album.rs` | 已归 `application` | 内测；`tests/sns_album_runtime.rs` 12 项 | **已迁移**；保持惰性媒体处理和来源保护 |
| [album_images.rs](../src/application/moments/album_images.rs) | 既有/远程图片选择、下载、可选前缀解密与受保护发布 | `album.rs` | 已归 `application`；微信密钥流归 adapters，文件机制归 infrastructure | 内测；`F/sns-album-images/` 16 项 | **已迁移**；仍非通用无授权下载器 |
| [album_videos.rs](../src/application/moments/album_videos.rs) | 既有/本地/远程视频准备、惰性前缀解密和有界流式发布 | `album.rs` | 已归 `application`；微信视频适配与发布机制复用现有实现 | 内测；`F/sns-album-videos/` 14 项 | **已迁移**；未改为整视频读入内存 |
| [album_render.rs](../src/application/moments/album_render.rs) | 相册数据投影、HTML 模板及转义 | `album.rs`、`D/sns_album.rs` | 已归应用呈现边界 | 内测；`F/sns-album-render/` | **已迁移**；仍是离线文档，不并入 Web server |
| [sns_keystream.rs](../src/adapters/wechat/media/sns_keystream.rs) | 固定摘要的 WxIsaac64 WASM 密钥流宿主，fuel/内存/输入预算；无网络/账号/文件系统宿主能力 | `D/sns_video.rs`、album 图片/视频层 | 微信 SNS 媒体适配；继续用现有 WASM 引擎，不新造通用脚本平台 | `sns_keystream_tests.rs`；`F/sns-video-native/`；`tests/asr_video_security.rs` | 已迁入微信媒体适配层，旧工作流模块移除；不是 AES 或视频帧解码器，沙箱边界保留 |

SNS 的 `*tests.rs` 随各自实现或目标适配器迁移；密钥流测试及 `SNS_KEYSTREAM.md` 已随微信媒体适配器迁移。解析 oracle 和相册/下载 fixture 不能因为删除旧脚本入口而退出回归范围。

## Web 子域

| 原模块 | 实际职责 | 当前调用入口 | 目标模块类别 | 对应现有回归测试 | 尚未迁移状态 |
| --- | --- | --- | --- | --- | --- |
| [web/mod.rs](../src/web/mod.rs) | Axum 路由、请求大小限制、HTTP 安全校验、SSE、静态资源与 serve 生命周期 | `src/cli/web_native.rs` | 顶层 `web` 入口；任务提交/取消继续访问 daemon service | 本文件 HTTP/资源内测；`F/daemon-tasks/web_query.rs` 是相关服务链线索 | **已迁移**；路由和生命周期保持原实现 |
| [query.rs](../src/web/query.rs) | 固定账号查询、有限并发槽位/等待、异步服务调用，不重放已发送请求 | Web 路由、preview、automatic_image | Web 的 service 客户端适配 | 本文件内测：队列有界、释放恢复、等待超时归还槽位；`web/mod.rs` 路由测试；`F/daemon-tasks/web_query.rs` | **已迁移**；仍只通过 service 层访问 daemon |
| [server_types.rs](../src/web/server_types.rs) | Web 实例共享状态、事件队列、日志限额及后台状态只读视图；Settings 重导出只是其中一项 | web/mod 和 query | 顶层 Web 的局部状态；Settings 所有权继续留现有 service | `web/mod.rs` 内测 | **已迁移**；未复制 Settings 契约 |
| [preview.rs](../src/web/preview.rs) | 预览列表/图片 RPC 转换，不枚举缓存或读本地文件 | Web preview 路由 → `service/web::Call` | Web 入口适配 | `web/mod.rs` 预览路由内测；daemon 对应媒体测试继续保留 | **已迁移**；仍为协议投影，不持有业务执行 |
| [automatic_image.rs](../src/web/automatic_image.rs) | 同源自动图片 HTTP 参数与响应转换，文件/密钥工作委托 daemon | Web 自动图片路由 → service Web Call | Web 入口适配 | `automatic_image_runtime_tests.rs` | **已迁移**；宿主文件和密钥操作仍在 daemon |
| [assets/](../src/web/assets) | 实际 HTML、JavaScript、CSS 与 Lucide 许可证 | `web/mod.rs` 的三个 `include_bytes!`；浏览器页面 | 顶层 Web 静态资源 | `web/mod.rs` 静态资源测试 | **已迁移**；资源和 Lucide 许可证完整保留 |

## 兼容与纯转发判定

以下判定只针对表中明确列出的文件/局部符号，不从目录或注释中的 legacy 推导整个域可以删除。

| 对象 | 明确源码证据 | 可执行的迁移结论 |
| --- | --- | --- |
| `asr/database_media.rs` | 完整文件只有说明和 `pub use crate::adapters::wechat::media::voice::*;` | **已完成**：调用者直达适配器，测试迁为 `adapters/wechat/media/voice_tests.rs`，转发文件已删除 |
| 原 `sns/decode.rs`、`sns/parse.rs` | 两个纯转发文件已删除 | 装配直接引用 moments 适配器；`html_entities.json` 已归属该适配器 |
| `emoticons/mod.rs`、`sns/mod.rs` | 前者仅声明 download；后者为模块声明和导出投影 | 是装配表，不是“子域没有功能”的证据；子模块迁完才能撤装配 |
| `mod.rs::parse_image_aes/parse_image_xor`、`sns/export.rs::load_comments` | 函数体直接委托 images 参数解析或 `adapters::wechat::moments::export_comments` | 可让调用者使用归位后的实际实现；不能据此删除同文件的报告或导出编排 |
| `legacy.rs`、`asr/local_python.rs` | 前者执行路径发现及受监督 Python 可用性检查；后者执行真实 Whisper/PyTorch 推理 | 属于兼容专用实现，不是空转发；在本轮“不能删功能”约束下必须迁移保留 |
| `asr/cached.rs`、`asr/windows_supervision.rs` | cached 会规范化 SILK、命中缓存、运行后端并提交结果；supervision 为 shared managed 增加调用者错误上下文 | 不按“薄适配”注释直接删除；分别归应用用例和后端辅助 |
| `sns/cache.rs` | re-export 之外还存在真实媒体恢复与状态实现 | 是混合职责模块，需按 SNS 表拆归属，不做整文件纯转发认定 |

## 分域迁移验收

以下为待执行的验收条件，不是新的完成记录。每个模块先保留上表对应的现有回归；没有直接覆盖的部分必须补用例，不能删断言或以 mock 成功替代真实入口验收。

| 适用模块/切片 | 必须保留的可观察行为 | 现有线索与仍缺的验收 |
| --- | --- | --- |
| 顶层装配、正式 CLI、批次结果 | 正式业务命令仍可到达应用用例；旧 migrate-keys/脚本入口仍拒绝；success/partial/failure 及 JSON/worker 输出语义不变 | `tests/current_entry_contract.rs`、`tests/entry_service_runtime.rs`、`T/mod.rs` 内测；目录迁移后要用当前 checkout 的受监督真实二进制验证 |
| configuration、private_file、publication、publication_context、output_tree | 旧配置拒绝不泄密、不改写；ACL 在首次敏感写入前生效；拒绝源/目标重叠与链接替换；失败不破坏既有文件；账号绑定和锁仍生效 | configuration、key_store、transport、publication、output_tree 的测试；迁移不能改变 key worker、租约或授权顺序 |
| 数据库和图片批次 | 数据库首页密钥验证、主文件/WAL 语义、缺钥与逐项失败计数不变；图片布局/扩展名/跳过行为及发布前复核不变 | `run_decrypt_runtime.rs`、native-image、native-attachment-security 和图片内测；dry-run 不产生导出文件 |
| chat_delta_export、chat_archive_merge、chat_archive_index、chat_directory | 跨分片身份稳定，歧义拒绝；旧消息/未知文件保留；同名联系人不串档；目录、媒体、文档投影与增量窗口契约不变 | delta/merge golden、delta runtime/security、chat_archive_index 内测、native migration security；纯规则迁移不能悄悄改变旧 JSON 格式 |
| chat_export_plan、chat_plan_selection | 只读统计与业务规则复用；CSV 选择只依赖 username；未知/歧义身份、数据库及输出越界均拒绝 | application、business、微信 planning adapter 内测；chat_plan_runtime、delta_plan_security、plan-selection、stable_username_roundtrip |
| cleanup 与 handles | 只处理计划明确选中的账号绑定文件；变更/未知文件保留；句柄确认同一文件对象；无授权不删除密钥或采用旧目录 | cleanup 内测继续保留；父代理边界收敛后补真实入口的计划/执行前后快照，不仅测路径字符串 |
| audio 编解码/批次/publish | SILK/WAV 输出一致；超时/输出超限回收进程树；WAV 不覆盖；宿主取消、身份变更和预算不足时不发布 | audio tests/process_tests/batch_tests、wav-publish、voice_runtime、daemon WAV 内测；Python oracle 不是生产转换器 |
| ASR 后端、缓存和 prepared audio | 本地/云端仅在显式选择后运行；后端失败不写成功缓存；prepared audio 不是成功转录；缓存及 receipt 不跨账号/身份复用；错误不泄露凭据 | local/openai/cache/cached/receipt/prepared 测试与 Python root_tests；远程测试用合成服务，不调用真实云端 |
| ASR batch/files/writeback | 快照/租约在批次期间有效；已完成缓存可恢复，但聊天 JSON 仅在符合发布条件时替换；不写微信原库 | writeback_tests 只覆盖共享回写；需补批次中途取消、源身份变化、缓存恢复与最终发布的直接回归 |
| SNS 解析、导出、缓存、媒体、相册、WASM | 单一 adapter/business 规则来源；作者/评论/时间线兼容和更新策略不变；下载授权/上限有效；视频沙箱能力和预算不扩大；离线 HTML 安全转义 | SNS tests、export update/download、album image/video/render、video runtime 及 SNS runtime 系列；实体资源和供应商资源路径一并验收 |
| SNS archive | 不能以已过滤 CacheIndex 替代完整枚举；失败项、缩略图、未知/冲突项可见且不误删；发布仍受来源约束 | 共享发布测试不足；需补直接归档样本和真实命令失败路径，文档不将此项标已覆盖 |
| monitor、state、transport、cancel、terminal、latency/statistics | 固定账号、不自动启停后台；完整游标分块；取消/断连释放暂存；损坏游标拒绝；终端转义和 JSONL 不变；客户端/后台耗时分别统计 | 保留 chunked/latency/statistics 内测；补轮询、状态保存、控制台卸载、同秒迟到和终端输出测试，不能声称可靠队列语义 |
| Web 全子域及资源 | HTTP 安全/同源约束、队列上限、SSE、任务提交/取消、预览与自动图片不回退到本地文件/密钥操作；资源可加载 | Web route/query/automatic-image 内测及 daemon Web 链路；还需迁移后页面操作验收，不把“链接存在”当成页面可用 |
| 纯转发、测试/静态资源 | 调用者直接依赖已有所有者；不再存在 Toolkit 生产、测试或 include 路径；同样的功能测试继续被注册 | 数据库语音转发与测试已完成；继续检查 html_entities、Web 三个嵌入资源和许可文件，以及独立 fixture 的 Cargo/build/harness 路径 |

## 删除目录的验收边界

1. **先等 keyworker 边界收敛。** 保留账号隔离、只读快照/租约、配置拒绝、取消与子进程回收、密钥权限及发布前复核；不得为消除引用先把这些检查移除。
2. **按用例迁移，而非整体重命名。** 先确定共享契约的所有者，再让 daemon、CLI、MCP、Web 使用应用入口；批处理结构化结果和现有 worker 输出/退出语义一起接续，禁止制造新的巨型 service。
3. **继续使用已外移实现。** `business` 和 `adapters/wechat` 已有功能不是待重写的占位；特别是计划、朋友圈查询、附件元数据、语音读取和图片批次规则，不能在新目录复制第二份。
4. **补齐资源和测试引用。** 检查生产 `toolkit::`、跨目录 `#[path]`、fixture harness/build 引用、`include_str!`/`include_bytes!` 和相关契约文档；数据库语音测试已迁移，`html_entities.json` 与 Web 资源仍是已证实的具体依赖，不仅是搜索建议。
5. **保留正式功能和安全断言。** 覆盖导出、增量/计划、数据库/图片/语音、ASR 后端与缓存、SNS 相册/下载/视频、清理/配置、监控、Web；旧 CLI 拒绝契约继续由现有测试守护。监控状态、控制台取消和完整轮询测试缺口须补齐，不能假定统计内测已经覆盖。
6. **最后才移除空目录。** 每行都有实际目标实现、调用更新和相关测试证据，且无剩余生产、资源或测试引用后，才能标“已迁移”。本文未执行这些步骤，也未启动任何测试或真实微信、模型、云调用。

### 交接时需复核

并行工作区会继续变化；执行后续迁移前应重新核对 `T/mod.rs`、各子域声明和调用者，尤其 `asr/batch`、图片和 SNS 与密钥及 worker 边界。若已有行在其他切片中完成，必须以新的源码和测试记录更新状态，不能以本文的目标路径推定迁移已经发生。
