# wx-workbench

微信本地数据工作台，命令为 `wx`。通过命令行、MCP 或本地 Web 界面查询、导出和整理已授权的微信数据。

支持 Windows x64 MSVC 与桌面微信 4.x；具体数据格式的适配范围取决于微信版本。本项目提供源码构建方式，npm 包尚未发布。

## 开发方式

本项目的开发代码由 AI 生成和修改，项目负责人未手工编写代码；人类负责需求、方向与验收。采用和参考的第三方代码与算法保留原作者归属，这一声明不表示上游代码也是 AI 原创。

## 主要能力

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

`key_store` 指向统一 DPAPI 加密存储，`keys_file` 是账号与存储绑定锚点。daemon 按需加载密钥，在内存中维护账号隔离的运行快照。配置文件不保存明文密钥。详见[密钥存储](docs/key-store.md)。

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

工具箱提供批量导出、增量、计划、音频处理和本地界面。`wx toolkit` 是批量处理和辅助工具的命令分组。所有子命令由 Rust 入口校验。具体参数以子命令 `--help` 为准。

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

普通查询读取本地数据。相册导出、媒体下载和时间线更新是独立操作，下载必须显式授权。离线视频的文件头检查不等于完整可播放性验证。参见[工作流条件](docs/workflow-requirements.md)和[密钥流宿主契约](src/adapters/wechat/media/SNS_KEYSTREAM.md)。

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

## 代码来源与致谢

本项目采用和参考了以下三个项目的代码与实现方法，感谢原作者及贡献者：

| 来源 | 采用和参考的内容 | 对本项目的意义 |
| --- | --- | --- |
| [jackwener/wx-cli](https://github.com/jackwener/wx-cli) | Rust CLI、daemon、数据库解密与缓存、联系人及聊天查询、附件处理、基础导出和 SNS 查询。 | 提供本地数据访问与查询执行的实现基础。 |
| [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) | 聊天批量与增量导出、朋友圈解析与导出、图片处理、语音转码与转录、表情索引与导出的实现及处理规则。 | 为导出工作流和媒体格式处理提供参考。 |
| [LOGO127/wechat-ai-memory](https://github.com/LOGO127/wechat-ai-memory) | Windows 账号密钥获取中的 SHA-512/HMAC 捕获方法与密钥派生方案。 | 为账号密钥适配提供参考。 |

第三方代码保留原作者归属；Rust 实现或 AI 修改不改变其来源。其他依赖、媒体资产参考及各项许可状态见[第三方说明](THIRD_PARTY_NOTICES.md)。

## 技术文档

[文档索引](docs/README.md)介绍本项目的架构、接口与使用条件。微信数据格式和机制研究另见 [wx-workbench-docs](https://github.com/leyan2174/wx-workbench-docs)；该仓库目前为 Private，需要访问权限。机制说明区分观察事实与推断，不属于微信官方规范。

## 贡献与安全报告

参与开发见[贡献说明](CONTRIBUTING.md)，安全问题见[安全报告](SECURITY.md)。不要公开真实账号、聊天、配置、密钥或完整调试日志。

## 许可

本项目原创部分采用 [Apache-2.0](LICENSE)，所采用的 wx-cli 代码保留 MIT 声明。部分第三方实现的授权证据尚待确认，不能将根许可证理解为对全部第三方材料的重新授权。第三方代码、资产及依赖遵循各自的许可证和版权声明，详见[第三方说明](THIRD_PARTY_NOTICES.md)。仅处理自己拥有或已获授权的数据。
