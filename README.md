# wx-workbench

微信本地数据工作台，命令为 `wx`。通过命令行、MCP 或本地 Web 界面查询、导出和整理已授权的微信数据。

支持 Windows 10 及以上的 x64 MSVC 环境与桌面微信 4.x；具体数据格式的适配范围取决于微信版本。本项目提供源码构建方式，npm 包尚未发布。

## 开发方式

本项目的开发代码由 AI 生成和修改，项目负责人未手工编写代码；人类负责需求、方向与验收。采用和参考的第三方代码与算法保留原作者归属，这一声明不表示上游代码也是 AI 原创。

## 主要能力

| 范围 | 能力 |
| --- | --- |
| 联系人与聊天 | 联系人、会话、历史、搜索、群成员、未读消息、收藏和统计 |
| 导出与整理 | 单聊及批量导出、增量导出、导出计划和附件提取 |
| 朋友圈与公众号 | 本地朋友圈查询、内容与相册导出、媒体处理、公众号文章查询 |
| 语音与媒体 | 图片解码、表情导出、原始 SILK 与关联 manifest 导出 |
| 使用入口 | 命令行、MCP、本地 Web 和实时监控 |

业务执行集中在账号隔离的 daemon 中。初始化、导出、媒体下载和清理有各自的前置条件，不由普通查询自动触发。查询和导出范围受本机已有数据库及媒体缓存限制；微信版本变化可能影响格式适配。

## 安装

### 准备 Windows 构建环境

目前仅支持 **Windows 10+ x64 / `x86_64-pc-windows-msvc`**，不支持 GNU 工具链或在 Linux、macOS、WSL 中直接构建。受控进程使用创建时 Job 绑定，不支持的系统或不兼容的宿主 Job 会明确拒绝创建，不退回创建后再绑定的路径。以下命令使用 PowerShell。

| 必需工具 | 安装与配置 |
| --- | --- |
| [Git for Windows](https://git-scm.com/downloads/win) | 用于下载源码，安装后确认 `git --version` 可执行。 |
| [Visual Studio / Build Tools](https://learn.microsoft.com/en-us/cpp/build/vscpp-step-0-installation) | 在 Visual Studio Installer 中选择“使用 C++ 的桌面开发”，安装 x64/x86 MSVC 构建工具及 Windows SDK。仅安装 VS Code 不包含这些工具。 |
| [Rust / rustup](https://rust-lang.org/tools/install/) | 使用 Windows x64 安装器；下方命令显式选择 stable MSVC 工具链。 |
| [LLVM](https://releases.llvm.org/) | 安装 Windows x64 版本，确认包含 `libclang.dll`；`frida-sys` 的构建依赖 `bindgen` 生成绑定时需要它。 |

安装后重新打开 PowerShell，准备工具链并检查环境：

```powershell
git --version
rustup --version
rustup toolchain install stable-x86_64-pc-windows-msvc
cargo +stable-x86_64-pc-windows-msvc --version
rustc +stable-x86_64-pc-windows-msvc --version

# 按本机 LLVM 安装位置修改；设置的是目录，不是 DLL 文件路径。
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
if (-not (Test-Path (Join-Path $env:LIBCLANG_PATH 'libclang.dll'))) {
    throw '未找到 libclang.dll，请检查 LLVM 安装位置'
}
```

`$env:LIBCLANG_PATH` 的设置只作用于当前终端；新开终端后需要重新设置，或自行加入用户环境变量。若 MSVC 工具未被找到，可从 Visual Studio 的 Developer PowerShell 中运行下方命令，并确保构建目标为 x64。

### 下载源码并编译

仓库为 Private 时，需要使用具有访问权限的 GitHub 账号完成 Git 身份认证；不要将访问令牌写进命令或配置示例。

```powershell
git clone https://github.com/leyan2174/wx-workbench.git
cd wx-workbench

cargo +stable-x86_64-pc-windows-msvc build --release --locked --bin wx --target x86_64-pc-windows-msvc
```

`--locked` 使用仓库的 `Cargo.lock`，避免构建时改变依赖版本；它不代表离线构建，也不固定 Rust 编译器版本。首次构建需要联网获取 Cargo 依赖及 Frida 原生开发包。SQLite、Frida、wasmi 等依赖由构建系统处理，不需要另外启动数据库服务或安装这几个项目的命令行工具。

默认产物为 `target\x86_64-pc-windows-msvc\release\wx.exe`。如果设置了 `CARGO_TARGET_DIR`，产物位于该目录下；不同 checkout/worktree 应使用独立构建目录。下面的命令兼顾默认目录和环境变量指定的目录（未通过 Cargo 配置或 `--target-dir` 另行覆盖）：

```powershell
$buildRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { '.\target' }
$binaryDir = Join-Path $buildRoot 'x86_64-pc-windows-msvc\release'
$wxExe = Join-Path $binaryDir 'wx.exe'
& $wxExe --version
& $wxExe --help

# 可选：仅为当前 PowerShell 会话加入 PATH，方便使用下文的 wx 命令。
$env:PATH = "$(Resolve-Path $binaryDir);$env:PATH"
```

`--version` 和 `--help` 用于验证程序能够启动，不要求账号初始化。查询真实数据前，还需按下文“选择账号”配置和初始化。源码构建不会自动发布 GitHub Release 或 npm 包。

### 检查与测试

开发或修改源码后，在仓库根目录、同一构建环境中执行：

```powershell
cargo +stable-x86_64-pc-windows-msvc check --locked --target x86_64-pc-windows-msvc
cargo +stable-x86_64-pc-windows-msvc test --locked --no-fail-fast --target x86_64-pc-windows-msvc -- --test-threads=1
```

独立夹具及需要人工条件的测试见[测试说明](tests/README.md)，完整质量检查见[质量检查](docs/quality-checks.md)。被忽略的用例不代表已经通过；不要为运行测试扫描真实账号、启动未经授权的捕获流程。

### 按功能准备运行依赖

以下组件不是编译 `wx.exe` 的前置条件，只在使用对应功能时准备：

| 功能 | 额外条件 |
| --- | --- |
| 普通查询、CLI、MCP、本地 Web | 不需要 Node.js、Python 或 FFmpeg；数据查询需要有效的账号配置与密钥。 |
| 表情、视频恢复中的媒体转换 | 配置可用的 FFmpeg；可先用 `ffmpeg -version` 检查。 |
| 依赖 WxIsaac64 的朋友圈媒体恢复 | 提供具有使用依据、符合固定哈希要求的 WASM 文件；默认构建和发布包不携带它，见[宿主说明](src/adapters/wechat/media/SNS_KEYSTREAM.md)。 |
| npm 启动器测试和打包 | 需要 Node.js / npm；直接构建 Rust 程序不需要。 |

缺少功能依赖时先完成相应配置，不以编译成功代替完整功能验证。

### 常见构建问题

| 现象 | 处理方式 |
| --- | --- |
| 找不到 `cargo` 或 `rustup` | 重新打开终端，检查 Rust 是否安装以及用户的 `.cargo\bin` 是否在 PATH 中。 |
| 找不到链接器、C/C++ 编译器或 Windows SDK | 在 Visual Studio Installer 中补齐 C++ 工作负载、MSVC 和 SDK；检查是否使用 MSVC x64 目标。 |
| `Unable to find libclang` 或无法加载 `libclang.dll` | 核对 x64 LLVM 和 `LIBCLANG_PATH`，确保该环境变量指向 DLL 所在目录。 |
| 下载 crates 或 Frida 开发包失败 | 检查网络、代理和相关下载站点的访问；首次构建不要使用 `--offline`，依赖准备齐全后再考虑离线构建。 |
| 提示仅支持 Windows x64 | 在原生 Windows 环境使用 `x86_64-pc-windows-msvc`，不要改用 GNU、ARM64 或 WSL 的 Linux 目标。 |
| 找不到生成的 `wx.exe` | 检查构建是否成功，以及 `CARGO_TARGET_DIR`、Cargo 配置或 `--target-dir` 是否改变了产物位置。 |
| 无法覆盖正在使用的 `wx.exe` | 关闭使用该构建产物的前台程序，并通过对应账号的 `wx daemon stop` 停止 daemon 后重试；不要按进程名批量结束其他账号的程序。 |

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

`config.json` 必须是 UTF-8 JSON，文件大小最多 4 MiB；超限在解析前拒绝，不会修改原配置。此限制针对程序配置，不是聊天数据库或导出文件的大小限制。

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
wx tags --json
wx tag-members '测试标签' --json
wx members $chat --json
wx unread --filter private,group --json
wx new-messages --json
wx stats $chat --json
wx favorites -n 20 --json
wx voice-messages $chat -n 20 --json
```

历史支持偏移、日期和类型筛选。`--type` 与 `--types` 不能同时使用；`--oldest-first` 从全部分片合并后的最早记录分页。默认取最新页，页内按时间展示。

CLI 日期采用本地时区，`--until YYYY-MM-DD` 包含当天最后一秒；MCP 和 HTTP 接收 Unix 秒。`link` 与 `file` 都映射到宽泛的应用消息类型 49，并非精确链接/文件分类。分页、类型及各入口的差异见[查询协议](docs/query-protocol.md)。

`--with-meta` 返回较重的来源与新鲜度信息。调试来源可能包含本地路径，不直接贴入公开报告。首次读取较大的数据库可能触发私有缓存准备；超时不表示无数据，也不构成自动重试写入操作的依据。

`wx monitor --help` 查看轮询参数。较大的增量状态通过认证服务分块传输，完整校验后只执行一次查询；不通过截断会话或拆分查询降低请求大小。状态限额、取消和计时字段见[监控入口](docs/daemon-entrypoints.md#监控与增量状态)。

## 消息与附件

转账、位置、引用和文件消息解码是读取消息结构，不执行转账或访问外部链接。消息 ID 跨分片可能重复，必要时提供时间戳；仍不唯一时返回歧义。

```powershell
wx decode-transfer $chat 123 1700000000 --json
wx decode-location $chat 123 1700000000 --json
wx decode-refer $chat 123 1700000000 --json
wx decode-file-message $chat 123 1700000000 --json
wx decode-record-item $chat 123 0 1700000000 --json
wx attachments --help
wx extract --help
wx voices --help
```

附件元数据、资源是否存在和明文导出是不同能力。缺失或匹配不唯一时，不伪造路径、大小或绑定证据。输出目录必须与账号源、缓存、配置和密钥分离。详见[附件契约](docs/native-attachment-contract.md)。

CLI 详情的时间戳省略或为 `0` 时不按时间筛选，仍要求消息唯一；HTTP 详情必须提交正数 `local_id` 和正数 `create_time` 精确定位。记录条目索引从 `0` 开始。只读详情不会下载或导出附件。

## 导出与账号维护

```powershell
wx export $chat --format markdown --output '.\output\chat.md'
wx --help
wx setup --help
wx cleanup --help
wx chats export --help
wx chats export-delta --help
wx chats plan --help
wx emoticons export --help
```

`wx chats` 提供批量导出、增量导出和导出计划，`wx emoticons` 提供表情导出，`wx setup` 和 `wx cleanup` 负责账号准备与清理。所有子命令由 Rust 入口校验。具体参数以子命令 `--help` 为准。

清理先预览，再按明确账号和文件选择执行。覆盖、下载和目录更新须分别获得授权；导出授权不等于修改原始微信数据库的授权。

## 朋友圈与公众号

```powershell
wx sns-feed -n 20 --json
wx sns-search '测试关键词' --json
wx sns-notifications --json
wx biz-articles -n 20 --json
wx sns-album --help
wx moments export-snapshot --help
wx media video decode --help
```

普通查询读取本地数据。相册导出、媒体下载和时间线更新是独立操作，下载必须显式授权。离线视频的文件头检查不等于完整可播放性验证。参见[工作流条件](docs/workflow-requirements.md)和[密钥流宿主契约](src/adapters/wechat/media/SNS_KEYSTREAM.md)。

## 原始语音

```powershell
wx voices --help
```

`voices` 导出原始 SILK，保留已有 `0x02` 前缀，不转换为 WAV/MP3；CLI `voice-messages`、MCP `get_voice_messages` 与 HTTP `/api/voice-messages` 提供只读语音目录查询，不读取音频正文。`voices` 的 `_voice_export_summary.json` 包含 `manifest`；完整聊天目录保留语音引用并生成 `_voice_manifest.json`，一起交给下游工具处理。

消息身份由精确会话和非零服务端 ID 组成，账号由 `account_id` 单独限定，不以 rowid 或媒体 ID 替代。语音已写出但关联未证明时，`voices` 仍报告 `partial`、`incomplete_items` 并以非零状态退出。字段与时间来源见[原始语音导出契约](src/business/VOICE_EXPORT.md#manifest-字段)。

本产品不提供语音识别、音频转码、模型管理或识别结果回写。详见[原始语音导出](src/business/VOICE_EXPORT.md)与[语音目录](docs/voice-catalog-boundary.md)。

## MCP、Web 与 daemon

```powershell
# MCP 必须显式设置 WX_CLI_CONFIG。
wx mcp
wx web
wx daemon status
wx daemon stop
```

MCP 使用逐行 JSON-RPC，标准输出只承载协议帧。初始化和工具列表不读取账号，业务由认证 daemon 执行。默认注册 23 项工具，包含查询及受控图片执行；未读、群成员、统计、收藏、公众号和朋友圈各有只读工具。任务工具由宿主另行启用，不算在默认 23 项内。工具参数不能设置账号或宿主输出根。当前查询工具与参数见[查询协议](docs/query-protocol.md)，会话和媒体宿主边界见[MCP 协议](src/mcp/PROTOCOL.md)。

Web 的资料查询面板接入搜索、未读、成员、统计、收藏、公众号、朋友圈和语音目录；聊天页的“记录范围”向服务端提交时间、类型和顺序，原本的本页筛选仍只作用于当前页。消息详情可查询引用、文件、合并记录条目、转账和位置。HTTP 参数与错误见[本地 HTTP API](docs/http-api.md)。当前 Web 接线不等于浏览器操作已验收，跨入口覆盖与剩余边界见[能力矩阵](docs/capability-matrix.md)。

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
| [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) | 聊天批量与增量导出、朋友圈解析与导出、图片处理、表情索引与导出的实现及处理规则。 | 为导出工作流和媒体格式处理提供参考。 |
| [LOGO127/wechat-ai-memory](https://github.com/LOGO127/wechat-ai-memory) | Windows 账号密钥获取中的 SHA-512/HMAC 捕获方法与密钥派生方案。 | 为账号密钥适配提供参考。 |

第三方代码保留原作者归属；Rust 实现或 AI 修改不改变其来源。其他依赖、媒体资产参考及各项许可状态见[第三方说明](THIRD_PARTY_NOTICES.md)。

## 技术文档

[文档索引](docs/README.md)介绍本项目的架构、接口与使用条件。微信数据格式和机制研究另见 [wx-workbench-docs](https://github.com/leyan2174/wx-workbench-docs)；该仓库目前为 Private，需要访问权限。机制说明区分观察事实与推断，不属于微信官方规范。

## 贡献与安全报告

参与开发见[贡献说明](CONTRIBUTING.md)，安全问题见[安全报告](SECURITY.md)。不要公开真实账号、聊天、配置、密钥或完整调试日志。

## 许可

本项目原创部分采用 [Apache-2.0](LICENSE)，所采用的 wx-cli 代码保留 MIT 声明。部分第三方实现的授权证据尚待确认，不能将根许可证理解为对全部第三方材料的重新授权。第三方代码、资产及依赖遵循各自的许可证和版权声明，详见[第三方说明](THIRD_PARTY_NOTICES.md)。仅处理自己拥有或已获授权的数据。
