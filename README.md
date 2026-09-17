# wx-workbench

微信本地数据工作台，原名 wx-cli。产品和 GitHub 仓库已更名为 `wx-workbench`，正式命令为 `wx`。本轮删除旧工具入口和旧密钥迁移机制，不承诺旧调用兼容；不会自动删除用户旧文件。参见[正式命名与安装方式](docs/project-naming.md)和[当前契约清理记录](docs/current-contract-cleanup-2026-09-16.md)。新 npm 包尚未发布。

从命令行、MCP 或本地 Web 界面查询、导出和整理自己的微信数据。支持 Windows x64 MSVC 与桌面微信 4.x。

项目在原版 [wx-cli](https://github.com/jackwener/wx-cli) 的 Rust 查询与解密基础上，吸收 [wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) 等开源项目的导出、媒体处理和微信数据解析能力，逐步完成 Rust 迁移与统一 daemon 执行。各部分来源见[工程演进](#工程演进)和[开源来源与致谢](#开源来源与致谢)。

## 开发方式

本项目当前开发与重构代码由 AI 生成和修改，项目负责人未手工编写代码；人类负责需求、方向与验收。继承、移植和参考的第三方代码与算法保留原作者归属，这一声明不表示上游代码也是 AI 原创。

## 主要能力

当前实现已删除 `src/toolkit`：应用工作流位于 `src/application`，微信格式位于 `src/adapters/wechat`，文件、音频、转录后端和进程能力位于 `src/infrastructure`。查询与独立 worker 均通过 daemon 密钥快照取得运行材料，应用工作流不选择持久化格式；初始化 bootstrap 仍由 daemon 创建正式存储。当前结构与待验证项见[架构说明](docs/architecture.md)。

| 范围 | 能力 |
| --- | --- |
| 联系人与聊天 | 联系人、会话、历史、搜索、群成员、未读消息、收藏和统计 |
| 导出与整理 | 单聊及批量导出、增量导出、导出计划和附件提取 |
| 朋友圈与公众号 | 本地朋友圈查询、内容与相册导出、媒体处理、公众号文章查询 |
| 语音与媒体 | 图片解码、表情导出、语音提取与转码、本地或云端转录 |
| 使用入口 | 命令行、MCP、本地 Web 和实时监控 |

业务执行集中在账号隔离的 daemon 中。初始化、导出、媒体下载、转录和清理有各自的前置条件，不由普通查询自动触发。查询和导出范围受本机已有数据库及媒体缓存限制；微信版本变化可能影响格式适配。

## 安装

构建需要 Rust MSVC 工具链、Visual Studio C++ 构建工具及 libclang。将 `LIBCLANG_PATH` 指向本机包含 libclang DLL 的目录。

```powershell
git clone https://github.com/leyan2174/wx-workbench.git
cd wx-workbench
cargo build --release --target x86_64-pc-windows-msvc
& .\target\x86_64-pc-windows-msvc\release\wx.exe --version
```

下文假定 `wx.exe` 已在 PATH，也可以直接调用上述文件。原生查询和 MCP 不需要 Node.js。MP3 编码需要 FFmpeg；转录依赖所选本地模型或显式授权的云端服务。依赖缺失时先处理前置条件，不自动安装、下载或切换云端。

## 选择账号

不同账号使用不同配置与运行目录。`synthetic-account` 是示例目录，不是真实账号。

```powershell
$account = Join-Path $env:USERPROFILE 'wx-workbench-data/synthetic-account'
$env:WX_CLI_CONFIG = Join-Path $account 'config.json'
$env:WX_CLI_HOME = Join-Path $account 'runtime'
wx init --help
```

首次初始化前确认 `$dbStorage` 是目标账号的实际 `db_storage` 目录：

```powershell
wx init --db-dir $dbStorage --key-provider memory
wx sessions --json
wx contacts -n 20 --json
```

普通查询需要已保存且适用于该账号的密钥。不要跨账号复用配置或密钥。provider、DPAPI 与显式重启捕获见[账号密钥](docs/account-key-provider.md)。只有用户明确授权后，才使用 `--force --key-provider account --restart-wechat`；该操作可能需要手机确认登录。

旧版密钥迁移入口已移除，旧文件不读取、不删除。核对选中账号配置后，通过 `wx init --force --key-provider memory` 显式获取当前密钥；旧图片密钥配置字段需先由用户处理。`key_store` 指向统一 DPAPI 存储，`keys_file` 仅保留为账号与存储绑定锚点。详见[密钥存储](docs/key-store.md)。

缓存命中仍检查当前源库首页，正式解密逐页认证；密钥失效或首页校验失败时不以旧缓存掩盖。主库与 WAL 在副本上完成后发布，但这不是在线账号的事务快照。详见[数据库认证边界](docs/architecture.md#数据库认证与缓存发布)。

配置、密钥、解密缓存、导出内容和运行令牌都是私人材料，不放入源码目录、版本控制或公开日志。

## 查询

先从会话列表选择普通联系人或群聊。`brandsessionholder` 等聚合入口不是普通聊天对象。显示名有歧义时，自动化优先使用查询返回的精确标识。

```powershell
$chat = '合成测试群'
wx sessions -n 20 --json
wx history $chat -n 50 --json
wx history $chat --types text,image --oldest-first -n 20 --json
wx search '测试关键词' --in $chat -n 20 --json
wx contacts -q '测试联系人' --json
wx members $chat --json
wx unread --filter private,group --json
wx new-messages --json
wx stats $chat --json
wx favorites -n 20 --json
```

历史支持偏移、日期和类型筛选。`--type` 与 `--types` 不能同时使用；`--oldest-first` 从全部分片合并后的最早记录分页。默认取最新页，页内按时间展示。

`--with-meta` 返回较重的来源与新鲜度信息。调试来源可能包含本地路径，不直接贴入公开报告。首次读取较大的数据库可能触发私有缓存准备；超时不表示无数据，也不构成自动重试写入操作的依据。

`wx toolkit monitor --help` 查看轮询参数。较大的增量状态通过认证服务分块传输，完整校验后只执行一次查询；不通过截断会话或拆分查询降低请求大小。状态限额、取消和计时字段见[监控入口](docs/daemon-entrypoints.md#监控与增量状态)。

## 消息与附件

转账、位置、引用和文件消息解码是读取消息结构，不执行转账或访问外部链接。消息 ID 跨分片可能重复，必要时提供时间戳；仍不唯一时返回歧义。

```powershell
wx decode-transfer $chat 123 1700000000 --json
wx decode-location $chat 123 1700000000 --json
wx attachments --help
wx extract --help
wx voices --help
```

附件元数据、资源是否存在和明文导出是不同能力。缺失或匹配不唯一时，不伪造路径、大小或绑定证据。输出目录必须与账号源、缓存、配置和密钥分离。详见[附件契约](docs/native-attachment-contract.md)。

## 导出与工具箱

```powershell
wx export $chat --format markdown --output '.\output\chat.md'
wx toolkit --help
wx toolkit setup --help
wx toolkit cleanup --help
wx toolkit export-chats-native --help
wx toolkit export-delta-native --help
wx toolkit chat-plan-native --help
wx toolkit export-emoticons --help
```

工具箱提供批量导出、增量、计划、音频处理和本地界面。`wx toolkit` 是当前正式命令分组，不是旧调用兼容层，也不对应 `src/toolkit` 目录。所有子命令由 Rust 入口校验。具体参数以子命令 `--help` 为准。

清理先预览，再按明确账号和文件选择执行。覆盖、下载、回写和目录更新须分别获得授权；导出授权不等于修改原始微信数据库的授权。

## 朋友圈与公众号

```powershell
wx sns-feed -n 20 --json
wx sns-search '测试关键词' --json
wx sns-notifications --json
wx biz-articles -n 20 --json
wx sns-album --help
wx toolkit export-sns-native --help
wx toolkit decode-sns-video --help
```

普通查询读取本地数据。相册导出、媒体下载和时间线更新是独立操作，下载必须显式授权。离线视频的文件头检查不等于完整可播放性验证。参见[工作流条件](docs/legacy-workflow-gap-audit.md)和[密钥流宿主契约](src/adapters/wechat/media/SNS_KEYSTREAM.md)。

## 语音与转录

```powershell
wx toolkit transcribe-audio-native --help
wx toolkit transcribe-chat-native --help
wx toolkit transcribe-database-native --help
```

SILK 解码与模型识别分开配置。本地 whisper.cpp 需要程序与模型；配置式 Python 依赖 Whisper/PyTorch，命名模型可能下载。云端转录必须明确授权上传，并提供端点、模型和凭据文件，缺少条件时不回退。

详见[本地 ASR](src/infrastructure/transcription/LOCAL.md)、[云端授权](src/infrastructure/transcription/OPENAI.md)、[缓存](src/application/transcription/CACHE.md)、[回写](src/application/transcription/WRITEBACK.md)与[批量音频](docs/voice-batch-export.md)。

## MCP、Web 与 daemon

```powershell
# MCP 必须显式设置 WX_CLI_CONFIG。
wx mcp
wx toolkit web
wx daemon status
wx daemon stop
```

MCP 使用逐行 JSON-RPC，标准输出只承载协议帧。初始化和工具列表不读取账号，业务由认证 daemon 执行。17 项注册工具包含只读查询及受控媒体执行，没有独立 stats 工具。工具参数不能设置账号、宿主输出根、模型或凭据。详见[MCP 协议](src/mcp/PROTOCOL.md)。

Web 是本地界面，不应暴露到不可信网络。只停止本任务创建且身份可验证的 daemon，不按进程名清理其他账号或用户应用。MCP 按操作短时固定配置，同一账号的密钥更新不需要关闭会话；替换配置或切换账号仍需重新连接。生命周期见[入口边界](docs/daemon-entrypoints.md)和[后台任务](docs/daemon-tasks.md)。

## 开发与测试

[文档索引](docs/README.md)提供技术导航，[架构](docs/architecture.md)解释职责，[测试说明](tests/README.md)列出自动测试与人工前置条件。

```powershell
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc
```

默认测试使用合成数据。需要扫码、手机确认、重启、补充凭据、购买服务或新下载授权的项目标记为跳过，不算通过。真实测试在仓库外使用私有配置和输出，公开报告只保留必要状态与计数。

## 工程演进

### 延续原版 wx-cli 的查询基础

本仓库从原版 wx-cli 的 Rust 实现继续演进，继承了命令行、daemon、数据库解密与缓存，以及联系人、会话、消息搜索、群成员、收藏、统计、基础导出和 SNS 查询等能力。Rust 实现和 daemon 架构在原版中已经存在。

### 接入 wechat-decrypt，再逐步迁移为 Rust

工具箱最初通过调用 wechat-decrypt 的 Python 入口，接入批量聊天导出、朋友圈导出、图片处理、语音转码与转录、表情处理等能力。随后，这些工作流逐步迁移到 Rust，并接入本项目的账号与执行体系。部分迁移使用 Python 参考实现进行行为对照测试。

原 `src/toolkit` 中的许多实现承接了 wechat-decrypt 的功能与处理规则，现已按应用、微信适配和基础设施职责归位。当前仓库不再携带或运行 wechat-decrypt 的 Python 源码副本；保留的 Python 仅是明确选择的 Whisper/PyTorch 推理后端，与旧业务脚本无关。

### 统一执行与完善工程可靠性

在继承和移植的基础上，本仓库逐步将 CLI、HTTP、MCP 的业务执行集中到 daemon，并完善后台任务、取消与关闭、账号隔离、查询状态和缓存管理。数据库页认证与 WAL 校验、导出发布、外部进程监督和本地 Web 访问控制也持续得到加强。

这一阶段的工作重点是让已有能力在多个入口下共享执行逻辑，并提高长时间运行、批量处理和失败恢复的可靠性。当前产品聚焦 Windows x64 MSVC。

## 开源来源与致谢

感谢以下项目及其贡献者对微信本地数据研究、工具实现和开源共享的投入。本项目涉及代码移植、行为迁移、方案借鉴及第三方资产复用，具体关系如下：

| 来源项目 | 本项目继承、移植或借鉴的内容 |
| --- | --- |
| [jackwener/wx-cli](https://github.com/jackwener/wx-cli) | 本仓库的原版基础：Rust CLI、daemon、解密缓存、联系人与聊天查询、基础导出、附件处理和 SNS 查询等。 |
| [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) | 工具箱的重要来源：聊天批量与增量导出、朋友圈解析与导出、图片解码及批量处理、语音转码与转录、表情索引与导出等功能及处理规则。监控、Web 和 MCP 也有对应的参考实现。大量能力由 Python 工作流迁移、适配到当前 Rust 模块中。 |
| [LOGO127/wechat-ai-memory](https://github.com/LOGO127/wechat-ai-memory) | Windows 账号密钥捕获所用的 SHA-512/HMAC 捕获思路及账号密钥派生方案。当前实现结合本仓库的 provider、账号绑定和密钥校验流程进行集成。 |
| [hicccc77/WeFlow](https://github.com/hicccc77/WeFlow)、[LifeArchiveProject/WeChatDataAnalysis](https://github.com/LifeArchiveProject/WeChatDataAnalysis) | 朋友圈视频的 WxIsaac64 媒体解码流程参考。固定哈希 WASM 的来源边界记录在 [媒体适配器资产说明](src/adapters/wechat/media/assets/README.md)；因未取得明确再分发依据，公开源码和发布包不携带该资产，加密单视频解码仅接受用户显式提供且哈希匹配的授权副本。 |

上述来源按能力说明，不代表当前参考副本中的每一行代码都来自外部项目，也不将迁移后的 Rust 实现视为相关功能或算法的首次创造。本仓库自身的改造主要集中于工程集成、平台适配、统一执行与可靠性。

同时感谢 Frida、SQLite、音频编解码、语音识别和 WASM 运行时等依赖的维护者。直接移植或借鉴的实现与普通库依赖分别保留其来源及已知许可状态，详见[第三方说明](THIRD_PARTY_NOTICES.md)。

## 源码可用性与技术文档

上述来源仓库可能已经无法访问，也可能在未来迁移、删除或停止公开；本项目同样无法保证仓库与源码始终可用。这里保留原始项目名称和链接，是为了记录技术来源与贡献归属，不代表这些链接始终有效。

截至 2026-09-15，GitHub 已公开针对原版 wx-cli 和 wechat-decrypt 的 DMCA 通知，两者访问均返回法律原因限制；WeFlow 页面仍可访问，但默认分支已移除原有实现并说明受到 DMCA 影响。其他来源的状态与证据见[DMCA 与文档发布风险](docs/dmca-and-publication-risk.md)。这些通知是权利主张与平台处理记录，不等于法院已经认定侵权。

因此，本项目除提供代码外，也将持续整理可以指导独立实现的技术文档，保存代码背后的知识。文档分为两类：

| 文档类别 | 说明内容 |
| --- | --- |
| wx-workbench 的实现原理 | 业务模型、模块职责、CLI/MCP/Web 与 daemon 的协作、任务调度、账号隔离、缓存、导出与错误处理，说明程序如何组织和运行。 |
| 微信本地数据的实现机制 | 根据已适配代码与验证结果归纳的数据库布局、表与字段关系、密钥获取与派生、数据库加密与 WAL、消息类型与内容编码、联系人及群成员映射、媒体关联与解码，以及朋友圈、语音和通话记录的识别与导出规则。 |

这些微信机制说明是对特定版本行为的研究记录，并非微信官方规范。文档将标注适用的微信版本与平台，区分代码可以证实的事实、实际验证结果和待确认的推断；不同版本的结构变化与适配差异也应明确记录。

文档的目标是：**在适用法律允许的范围内，即使原始仓库和本项目源码都无法取得，开发者仍能依据保存下来的说明，独立重建相应功能的程序。** 为此，说明需要包含必要的数据结构、字段语义、算法参数、处理步骤、输入输出、边界条件和可复现的测试样例，而不只是介绍文件名称或要求读者“参考源码”。这一目标不构成源码或文档可以永久公开的承诺。

这是一项持续完善的工作，并不表示现有文档已足以完整重建所有功能。已整理的内容从[文档索引](docs/README.md)进入；后续将逐步补充各版本的机制说明、验证样例与尚未覆盖的部分。独立实现时仍需验证目标微信版本，并保留所复用代码或资产的来源与许可声明。

### DMCA 与公开发布的风险

DMCA 是美国《数字千年版权法》（Digital Millennium Copyright Act）。除版权通知与下架程序外，它还包含针对规避技术保护措施的规定。已公开的微信相关通知重点涉及密钥提取和数据库解密；本仓库包含相近能力并吸收了相关实现，因此存在实质性的投诉与下架风险。开源许可证、语言重写、“仅供研究”或“只处理自己的数据”声明，均不能单独排除这类争议。是否违法或适用例外，需要结合事实和适用法律判断。

**技术文档同样可能受到影响。** 整个仓库被禁用时，同仓库中的 README 和文档也会失去公开访问入口；文档另行发布并不自动获得法律豁免。用原创文字解释数据格式、接口和一般原理，与提供可直接重建密钥提取或保护措施规避能力的详细步骤，应分别评估。文档不会仅因采用 Markdown、伪代码或自然语言而免受投诉，也不会仅因介绍原理就当然违法。

项目将按内容分别评估技术说明的发布范围；如收到通知，应结合具体指控、平台流程及专业法律意见处理，不将文档作为保证继续传播被投诉内容的替代途径。详见[核查记录与发布边界](docs/dmca-and-publication-risk.md)、[GitHub DMCA 政策](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy)和[美国版权局第 1201 条说明](https://www.copyright.gov/1201/)。

## 贡献与安全报告

参与开发见[贡献说明](CONTRIBUTING.md)，安全问题见[安全报告](SECURITY.md)。不要公开真实账号、聊天、配置、密钥或完整调试日志。

## 许可

本项目原创部分采用 [Apache-2.0](LICENSE)，原版 wx-cli 的 MIT 声明完整保留。发布准备中仍有上游衍生实现的授权证据待确认，不能将根许可证理解为对全部第三方材料的重新授权。第三方代码、资产及依赖遵循各自的许可证和版权声明，详见[第三方说明](THIRD_PARTY_NOTICES.md)。仅处理自己拥有或已获授权的数据。
