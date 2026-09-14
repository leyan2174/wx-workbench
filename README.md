# wx-cli

从命令行、MCP 或本地 Web 界面查询、导出和整理自己的微信数据。支持 Windows x64 MSVC 与桌面微信 4.x。

会话、历史、搜索、联系人、群成员、收藏、统计、朋友圈、附件和语音查询由账号隔离的 daemon 执行。初始化、导出、媒体发布、转录和清理有各自的授权与输出条件，不由普通查询自动触发。

## 安装

构建需要 Rust MSVC 工具链、Visual Studio C++ 构建工具及 libclang。将 `LIBCLANG_PATH` 指向本机包含 libclang DLL 的目录。

```powershell
git clone https://github.com/lvsong/wx-cli.git
cd wx-cli
cargo build --release --target x86_64-pc-windows-msvc
& .\target\x86_64-pc-windows-msvc\release\wx.exe --version
```

下文假定 `wx.exe` 已在 PATH，也可以直接调用上述文件。原生查询和 MCP 不需要 Node.js。MP3 编码需要 FFmpeg；转录依赖所选本地模型或显式授权的云端服务。依赖缺失时先处理前置条件，不自动安装、下载或切换云端。

## 选择账号

不同账号使用不同配置与运行目录。`synthetic-account` 是示例目录，不是真实账号。

```powershell
$account = Join-Path $env:USERPROFILE 'wx-cli-data/synthetic-account'
$env:WX_CLI_CONFIG = Join-Path $account 'config.json'
$env:WX_CLI_HOME = Join-Path $account 'runtime'
wx init --help
```

首次初始化前确认 `$dbStorage` 是目标账号的实际 `db_storage` 目录：

```powershell
wx init --db-dir $dbStorage
wx sessions --json
wx contacts -n 20 --json
```

普通查询需要已保存且适用于该账号的密钥。不要跨账号复用配置或密钥。provider、DPAPI 与显式重启捕获见[账号密钥](docs/account-key-provider.md)。只有用户明确授权后，才使用 `--force --key-provider account --restart-wechat`；该操作可能需要手机确认登录。

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

工具箱提供批量导出、增量、计划、音频处理和本地界面。兼容命令名仍由当前 Rust 入口校验，不把未知命令交给任意脚本。具体参数以子命令 `--help` 为准。

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

普通查询读取本地数据。相册导出、媒体下载和时间线更新是独立操作，下载必须显式授权。离线视频的文件头检查不等于完整可播放性验证。参见[工作流条件](docs/legacy-workflow-gap-audit.md)和[视频契约](src/toolkit/sns/VIDEO_RUNTIME.md)。

## 语音与转录

```powershell
wx toolkit transcribe-audio-native --help
wx toolkit transcribe-chat-native --help
wx toolkit transcribe-database-native --help
```

SILK 解码与模型识别分开配置。本地 whisper.cpp 需要程序与模型；配置式 Python 依赖 Whisper/PyTorch，命名模型可能下载。云端转录必须明确授权上传，并提供端点、模型和凭据文件，缺少条件时不回退。

详见[本地 ASR](src/toolkit/asr/LOCAL.md)、[云端授权](src/toolkit/asr/OPENAI.md)、[缓存](src/toolkit/asr/CACHE.md)、[回写](src/toolkit/asr/WRITEBACK.md)与[批量音频](src/toolkit/audio/BATCH.md)。

## MCP、Web 与 daemon

```powershell
# MCP 必须显式设置 WX_CLI_CONFIG。
wx mcp
wx toolkit run web
wx daemon status
wx daemon stop
```

MCP 使用逐行 JSON-RPC，标准输出只承载协议帧。初始化和工具列表不读取账号，业务由认证 daemon 执行。17 项注册工具包含只读查询及受控媒体执行，没有独立 stats 工具。工具参数不能设置账号、宿主输出根、模型或凭据。详见[MCP 协议](src/mcp/PROTOCOL.md)。

Web 是本地界面，不应暴露到不可信网络。只停止本任务创建且身份可验证的 daemon，不按进程名清理其他账号或用户应用。活动 MCP 会话中不替换配置；切换账号使用新会话。生命周期见[入口边界](docs/daemon-entrypoints.md)和[后台任务](docs/daemon-tasks.md)。

## 开发与测试

[文档索引](docs/README.md)提供技术导航，[架构](docs/architecture.md)解释职责，[测试说明](tests/README.md)列出自动测试与人工前置条件。

```powershell
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc
```

默认测试使用合成数据。需要扫码、手机确认、重启、补充凭据、购买服务或新下载授权的项目标记为跳过，不算通过。真实测试在仓库外使用私有配置和输出，公开报告只保留必要状态与计数。

## 许可与致谢

项目采用 [Apache-2.0](LICENSE)。本项目受 [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) 启发，在其基础上重新设计与实现。感谢原作者的研究与探索。

账号捕获和依赖的来源、版权与许可见[第三方说明](THIRD_PARTY_NOTICES.md)。仅处理自己拥有或已获授权的数据。
