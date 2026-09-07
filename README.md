<div align="center">

# wx-cli

**从命令行查询、导出和整理本地微信数据**

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20x64-lightgrey.svg)](#安装)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)

会话 · 聊天记录 · 搜索 · 联系人 · 群成员 · 朋友圈 · 附件 · 语音 · 导出

</div>

---

## 当前实现

### 转账消息解码

```powershell
wx decode-transfer "联系人" 123
wx decode-transfer "联系人" 123 1700000000 --json
```

该命令为 `wechat-decrypt/decode_transfer.py` 的原生 Rust 移植，输出状态、展示金额、备注、付款/收款账号、交易号与时间。
同一消息 ID 出现在多个分片时不会任取一条，需提供时间戳；仍不唯一时退出码为 2，普通解码错误为 1。
聊天记录中的转账摘要也复用同一领域解析模块；缺失金额不会推算成零。这个功能不需要 Python。

### 全系统状态

个人微信及通用基础功能的本轮 Rust 重构、最终精简和自动化回归已收尾。
数据库与图片解密、聊天/朋友圈导出、计划与增量、自动语音关联和转录编排、MCP、配置向导、监控以及本地 Web/GUI 均有 Rust 生产入口。GUI 是打开浏览器的本地工作台，不是旧 Python 桌面窗口的逐像素复刻。

直接运行 `wx.exe` 不需要 Node。SILK 解码原生实现，MP3 编码和部分 HEVC 处理需要 FFmpeg；模型推理可选 whisper.cpp、Python Whisper/PyTorch 兼容后端或明确授权的云后端。旧批量转录按账号配置选择引擎，缺省 `transcription_backend="local"` 使用 Python；MCP 的 Python 路径必须由宿主显式开启。浏览器脚本、Frida Hook、WASM、npm 包装与测试对照资产有明确用途，不以删除它们冒充“纯 Rust”。

企业微信按用户要求排除本次移植验收，已有入口保留但不宣称其功能完整。真实账号完整历史、任意媒体播放、实际模型/GPU、云服务和安装部署仍需独立验证；最新测试及警告数量统一记录在迁移验收文档，不以历史测试快照代表当前状态。

- [文档索引与适用范围](docs/README.md)
- [当前架构与数据流](docs/architecture.md)
- [Daemon 任务服务与取消恢复](docs/daemon-tasks.md)
- [功能迁移与回归清单](docs/rust-migration.md)
- [wechat-decrypt 完整源码能力盘点](docs/legacy-capability-inventory.json)

后台运行环境已按账号和配置工作区隔离，缓存、管道、PID、日志统一位于
`WX_CLI_HOME/accounts/<运行身份>/` 对应命名空间。命名管道本身由 Windows 管理，不是该目录中的文件。
可用 `WX_CLI_CONFIG` 固定配置文件；旧缓存与旧版固定管道不会自动删除或接管。
显式 `WX_CLI_HOME` 的配置发现与初始化写入位置保持一致，不回退到默认用户目录的账号。
查询管道业务请求默认 300 秒超时，可用 `WX_CLI_REQUEST_TIMEOUT_SECS` 设置更长期限；Ping 固定 1 秒。独立任务 RPC 使用 10 秒总超时，不限制后台任务总运行时长。

常规原生批处理读取当前 wx-cli 账号配置，输出 JSON 统计；有失败项时返回非零退出码。企业微信和离线 SNS 命令使用显式传入的数据库路径，不推断账号。
数据库导出只包含主文件快照，不合并 WAL，也不会自动扫描密钥。
批处理输出目录必须位于源目录之外，路径不接受 `..`；写入失败保留已有结果。
普通目录批量解图保留去除 `_t/_h` 后缀及跳过已有输出的行为。

## AI Agent Skill

通过 [skills CLI](https://github.com/vercel-labs/skills) 一键安装到 Claude Code、Cursor、Codex 等 agent：

```bash
npx skills add lvsong/wx-cli
```

或全局安装：

```bash
npx skills add lvsong/wx-cli -g
```

安装后 agent 会自动读取 `SKILL.md`，了解如何安装和调用 wx-cli。

---

## 特性

- **原生入口**：Windows x64 Rust 二进制；额外媒体与模型依赖按功能选择
- **后台缓存**：daemon 持久缓存解密数据库，mtime 不变则复用；响应时间取决于数据量与缓存状态
- **AI 友好** — `history` / `search` / `sessions` / `new-messages` / `stats` / `attachments` 默认返回 `{..., meta}` wrapper，agent 能直接消费 freshness / source 信息
- **本地查询**：默认在本机查询与解密；显式云 ASR 会上传音频，授权 SNS 下载会连接媒体服务器
- **语音原始导出** — 从微信媒体数据库导出 `.silk`，同时生成可追溯的 `.voice.json`
- **工具箱集成**：Rust 统一编排数据库、图片、朋友圈、语音和本地工作台；第三方源码保留作参考及特定资产来源

### lvsong 增强内容

Windows 现支持可选的账号级密钥捕获：

```powershell
wx init --force --db-dir "<账号>\db_storage" --key-provider account --restart-wechat
```

该命令会重启微信并等待重新登录，通过 Rust + Frida 获取账号原始派生密钥，
逐库校验后以 Windows DPAPI 加密保存为配置目录下的 `account_key.dpapi`。
以后普通 `wx init --force` 会优先尝试复用，新增数据库也会逐库派生并验证。
`--key-provider memory` 可强制使用原有只读扫描。此提供器无需 Python/Node.js，
但仍包含运行在 Frida 内置引擎中的 Hook JavaScript；其他媒体能力的依赖见上方说明。
用法、构建依赖与验证边界见 [账号级密钥提供器](docs/account-key-provider.md)。

本仓库在原版 `wx-cli` 基础上增加了 Windows 微信资料整理所需的两组能力：

- `wx voices`：直接从 `message/media_*.db` 导出语音原始数据；
- `wx toolkit`：通过同一个 `wx` 命令调用移植后的 Rust 工作流。

当前增强版版本号为 `0.3.0-leyan.7`。

Windows 命令启动时会清除原始标准句柄的可继承标记，防止微信或
wx-daemon 在后台运行时占住调用脚本的输出管道。命令完成后可正常返回，
无需关闭微信或停止 daemon；显式子进程输出重定向仍然有效。

Windows 微信 4.1.12.26 的新版密钥提供器曾在记录所列构建上完成本机端到端验证，不代表本次构建重新验收过私人账号，详见
[Windows 新版微信密钥导出验证记录](docs/windows-wechat-4.1-key-provider-verification.md)。

---

## 安装

本版本仅支持 Windows x64（MSVC），其他平台实现及发布包已移除。
构建需安装 Rust、Visual Studio C++ 构建工具和 libclang；详见
[构建依赖](docs/account-key-provider.md#build-and-validation)。

**从源码构建增强版（推荐）**

```bash
git clone https://github.com/lvsong/wx-cli.git
cd wx-cli
cargo build --release --target x86_64-pc-windows-msvc
```

构建产物：

```text
target/x86_64-pc-windows-msvc/release/wx.exe
```

验证增强命令：

```bash
wx --version
wx voices --help
wx toolkit status --json
```

> `@jackwener/wx-cli` npm 包属于原始基础版，不包含本仓库新增的
> `voices` 和 `toolkit` 功能。

<details>
<summary>其他安装方式</summary>

**手动下载**

从 [Releases](https://github.com/lvsong/wx-cli/releases) 下载对应平台文件：

| 平台 | 文件 |
|------|------|
| Windows x86_64 | `wx-windows-x86_64.exe` |


</details>

---

## 快速开始

保持微信运行，然后初始化（只需一次）：

**Windows**（以管理员身份运行 PowerShell）

```powershell
wx init
```

验证安装：

```bash
wx sessions
```

能看到最近会话说明基础账号查询可用，不代表全历史、媒体或转录均已验证。daemon 在首次调用时自动启动。源码更新不会自动替换 PATH 中的旧程序，验证时先确认实际调用的是新构建的 `wx.exe`。

---

## 命令

### 后台任务

```powershell
wx tasks info
wx tasks submit export-all --users "synthetic-user" --formats json --no-images --wait
wx tasks list
wx tasks logs <任务ID> --follow
wx tasks cancel <任务ID>
```

`wx tasks` 与 Web 共用 daemon 中的 12 类任务，关闭 Web 或等待客户端不会取消任务；`wx daemon stop` 才会请求后台停机并回收任务进程。设置冲突、幂等 ID、日志截断和恢复边界见[任务服务说明](docs/daemon-tasks.md)。旧 toolkit 直接命令及 MCP 保留原入口，不是所有 CLI 参数的任务化镜像。

### 消息

```bash
wx sessions                                      # 最近 20 个会话
wx unread                                        # 有未读消息的会话
wx unread --filter private,group                 # 只看真人未读（过滤公众号/折叠入口）
wx new-messages                                  # 上次检查后的新消息（增量）
wx history "张三"                                # 最近 50 条记录
wx history "张三" -n 2000                        # 拉更多历史消息
wx history "AI群" --since 2026-04-01 --until 2026-04-15
wx history "AI群" --types text,image --oldest-first -n 50
wx search "关键词"                               # 全库搜索
wx search "关键词" -n 500                        # 放宽搜索结果条数
wx search "会议" --in "工作群" --since 2026-01-01
```

`history` / `search` / `export` 都支持 `-n` / `--limit` 指定条数。默认值只是为了避免一次性输出过多消息，不是硬上限。

会话/消息输出里都带 `chat_type` 字段，取值为 `private` / `group` / `official_account` / `folded`。`official_account` 涵盖公众号、订阅号、服务号及 `mphelper` / `qqsafe` 等系统通知；`folded` 对应微信里的"订阅号折叠"和"折叠群聊"两个聚合入口。

群聊里的 `last_sender`、`sender` 和 `stats` 的 `top_senders` 会优先使用群昵称（群名片）。如果本地数据库里没有对应群昵称，则回退到联系人备注、微信昵称或 username。

`history` / `search` / `new-messages` / `attachments` 以及 `stats.top_senders`，在群聊上下文里还会附带稳定身份三件套：

- `sender_username`：稳定 wxid，用来区分两个昵称同名的成员
- `sender_contact_display`：通讯录里的显示名（备注 > 昵称 > wxid 兜底）
- `sender_group_nickname`：群名片本身（同 `sender` 的来源，方便机器读取时不必再解析）

解析不到 wxid 时（id2u 没命中且老格式 `wxid_xxx:\n...` 前缀也不存在）这三字段不会输出，避免伪造空字段污染下游过滤。

`history` / `search` / `sessions` / `unread` / `new-messages` / `stats` / `attachments` 现在都会附带 `meta`：

- `status`: `ok` / `possibly_stale` / `possibly_stale_unknown_shards` / `windowed`
- `unknown_shards`: 磁盘上存在、但 daemon 当前没有 key 的 `message_N.db` 分片；非空时应先跑 `wx init --force`
- `chat_latest_timestamp` / `chat_latest_db`: 当前命中数据里最新一条消息的时间和分片来源
- `session_last_timestamp`: `session.db` 里 WeChat 自己记录的最新时间；如果明显领先于 `chat_latest_timestamp`，说明结果可能漏了消息

默认情况下，人类用户会在 stderr 看到可执行的 warning；agent / 脚本可直接读 stdout 里的 `meta`。传 `--with-meta` 会额外返回 `per_shard_latest` / `cache_mode_per_shard`，传隐藏 flag `--debug-source` 还会带真实 `shard_paths`。

引用消息会在 `history` / `search` / `new-messages` 输出中显示当前回复和被引用原文：

```text
[引用] 当前回复
  ↳ 发送者: 被引用内容
```

`--type link` / `--type file` 会包含微信 appmsg 里的链接、文件、合并聊天记录和引用消息等变体；搜索时也会匹配解压后可见的引用原文。

### 朋友圈（SNS）

三个独立命令，区分"通知"和"帖子"：

```bash
wx sns-notifications                             # 点赞/评论通知（默认仅未读）
wx sns-notifications --include-read -n 100       # 含已读

wx sns-feed                                      # 近 20 条朋友圈（时间线）
wx sns-feed --user "张三"                        # 限定作者
wx sns-feed --since 2026-04-01 -n 100            # 按时间

wx sns-search "关键词"                           # 全文搜索朋友圈正文
wx sns-search "婚礼" --user "李四" --since 2023-01-01

# 导出指定联系人的文字、图片、视频和手机相册 HTML
wx sns-album "张三" -o ./moments-export
wx sns-album "张三" -o ./moments-export --since 2025-01-01 --until 2025-12-31
wx sns-album "张三" -o ./moments-export --no-videos
wx sns-album "张三" --output-dir ./existing-album --no-remote
wx sns-album "张三" --output-dir ./legacy-album --adopt-existing
```

- **sns-notifications** 返回互动通知：`type`（`like`/`comment`）、`from_nickname`、`content`（评论正文）、`feed_preview` + `feed_author`（对应原帖）
- **sns-feed** / **sns-search** 返回朋友圈帖子：`post_id`、`author`、`content`（正文）、`media`、`media_count`、`location`、`timestamp`；`media` 字段含媒体 `id`、url/thumb/key/token/md5/enc_idx/size，视频另含 `enc_key`，供下游做媒体恢复或离线渲染。`media_count = media.len()`，按 DOM 解析的合法 `<media>` 子节点计数（malformed XML 返回 0）

朋友圈数据只覆盖你本地刷到过的帖子（微信 app 按需下载）。

`sns-album` 会在输出根目录下创建带时间戳的相册目录：

```text
张三-朋友圈相册-YYYYMMDD-HHMMSS/
├── timeline.json       # 完整结构化记录和媒体元数据
├── timeline.html       # 可离线打开、兼顾手机阅读的相册
├── export_summary.json # 图片、视频成功与缺失统计
├── _source_binding.json # 账号和联系人绑定，不是导出完成标记
├── images/             # 已有图片复用或 CDN 下载、解密
└── videos/             # 本地缓存或 CDN 解密恢复的 MP4
```

`sns-album` 使用 Rust 编排，不启动 Python 或 Node。图片依次复用相册中已有的有效图片、尝试原图 URL、尝试缩略图 URL，支持 token/密钥候选；不扫描图片缓存，摘要中的 `image_cache` 保持旧相册逻辑的 0。视频依次复用已有视频、使用当前账号 `cache/YYYY-MM/Sns/Video` 的完整缓存、下载远程视频，最后才保留可用的部分缓存。部分缓存单独计数。

解密通过 Rust wasmi 懒加载内嵌 WASM：图片最多 25 MiB，视频只解密前 128 KiB，其余流式复制，单视频上限 2 GiB。MP4 文件头和来源状态不证明容器完整或一定可播放。`--no-remote` 禁止图片和视频下载，仍复用已有媒体及本账号视频缓存；`--no-videos` 跳过视频。`--image-workers` 默认 8（限制到 1–32），`--video-workers` 默认 4（限制到 1–16）。HTML 只显示有正文或已恢复媒体的记录；`timeline.json` 保留全部查询结果，不等于完整历史。

`-o/--output/--output-root` 创建带时间戳的新目录；`--output-dir` 更新已绑定同一账号和联系人的相册，并保留无关旧文件。非空、无绑定的旧目录必须显式加 `--adopt-existing`；已有绑定冲突不能强行接管。认领后摘要持续标记 `legacy_unverified`，不证明旧媒体的历史来源。媒体、JSON、HTML、摘要按顺序逐文件原子替换，整个相册不是事务，中途失败可能部分提交；来源绑定文件不是成功标记。媒体缺失计入摘要，输出或来源保护失败则命令失败。

Feed 使用固定账号后台，最多六次只读尝试，每次 30 秒、响应上限 256 MiB。旧后台缺少精确作者元数据时，需要用当前版本重启该账号后台。扫描达到 50000 条上限会在摘要警告，不能据 `posts` 数量推断完整在线历史。

该命令不会主动向微信服务器拉取历史朋友圈。若目标记录还没有出现在 `wx sns-feed --user "张三"` 中，需要先在微信客户端打开该联系人的朋友圈并加载相应时间范围。

### 公众号文章

公众号文章推送存在独立的 `biz_message_*.db` 分片，用 `biz-articles` 单独查：

```bash
wx biz-articles                                   # 最近 50 篇
wx biz-articles -n 200                            # 更多
wx biz-articles --account "返朴"                  # 限定公众号（名称模糊匹配）
wx biz-articles --since 2026-05-01 --until 2026-05-10
wx biz-articles --unread                          # 仅有未读的公众号，每号取最新 1 篇
wx biz-articles --json | jq '.[].url'             # 下游消费 URL
```

每条返回：`account` / `account_username` / `title` / `url` / `digest` / `cover_url` / `time` / `timestamp` / `recv_time_str`。多图文推送会展开成多行。

### 附件提取（图片）

聊天里的附件本体存在 `xwechat_files/<wxid>/msg/attach/...` 下的 `.dat` 文件，需要按消息所在 `message_resource.db` 的 md5 + 平台相关 image key 解码才能拿到原图。

```bash
# 1) 列出会话里的图片附件，先拿到不透明的 attachment_id
wx attachments "张三"
wx attachments "AI群" --kind image -n 100
wx attachments "AI群" --since 2026-04-01 --until 2026-04-15

# 2) 把单个 attachment_id 解密写出去（扩展名建议保留 .jpg / .mp4 等）
wx extract <attachment_id> -o ~/Desktop/photo.jpg
wx extract <attachment_id> -o /tmp/x.jpg --overwrite
```

`attachments` 输出每条带：`attachment_id` / `kind` / `type` / `local_id` / `timestamp` / `time`，群聊里还有 `sender` 以及稳定身份三件套 `sender_username` / `sender_contact_display` / `sender_group_nickname`（语义同 `history` / `search` / `new-messages`：`sender_username` 是 wxid，用于两个同名成员之间的稳定区分；解析不到 wxid 时这三字段不输出）。当前 `kind` 固定为 `image`；命令名保留成 `attachments` 是为了后续扩到其他附件类型时不 break CLI。

`extract` 输出报告里带：`md5` / `dat_path` / `dat_size` / `output` / `output_size` / `format`（实际识别出的图片格式：jpg / png / gif / webp / hevc 等）/ `decoder`（实际选用的解码器：`legacy_xor` / `v1_aes` / `v2`）。

支持的解码档位：
- **legacy XOR**：早期单字节 XOR，无 magic（按文件首字节探测格式自动反推）
- **V1 fixed-AES**（`07 08 V1 08 07`）：AES-128-ECB + 固定 key `cfcd208495d565ef`
- **V2 AES + XOR**（`07 08 V2 08 07`）：AES-128-ECB + raw + XOR；AES key 平台派生

V2 image key 提取：
- **Windows**：扫 `Weixin.exe` 内存匹配 `[A-Za-z0-9]{32|16}` 候选，按 V2 template ciphertext-block 反验

### 联系人 & 群组

```bash
wx contacts                  # 联系人列表
wx contacts --query "李"     # 按名字搜索
wx members "AI交流群"        # 群成员列表
```

`wx members --json` 返回的成员字段包括：

- `username`：微信内部 username
- `display`：用于展示的名称，优先使用群昵称
- `contact_display`：联系人备注或微信昵称
- `group_nickname`：群昵称；本地没有记录时为空字符串
- `is_owner`：是否群主

### 收藏 & 统计

```bash
wx favorites                          # 全部收藏
wx favorites --type image             # 按类型筛选（text/image/article/card/video）
wx favorites --query "关键词"         # 搜索收藏内容
wx stats "AI群"                       # 聊天统计
wx stats "AI群" --since 2026-01-01   # 指定时间范围
```

### 导出

```bash
wx export "张三" --format markdown -o chat.md
wx export "张三" -n 2000 --format markdown -o chat.md
wx export "AI群" --since 2026-01-01 --format json
```

### 输出格式

普通查询默认输出 YAML；`--json` 可切换为 JSON。工具箱批处理通常输出 JSON 报告，MCP 使用 JSON-RPC，Web/GUI 是持续服务。对 agent 而言，`history` / `search` / `sessions` / `new-messages` / `stats` / `attachments` 的 stdout 是 wrapper，而不是裸数组：

```bash
wx sessions --json
wx search "关键词" --json | jq '.results[0].content'
wx new-messages --json
wx history "张三" --json | jq '.meta'
wx history "张三" --json --with-meta | jq '.meta.cache_mode_per_shard'
```

### Daemon 管理

```bash
wx daemon status
wx daemon reload
wx daemon stop
wx daemon logs --follow
```

`wx daemon reload` 会丢弃并重新解密联系人数据库缓存，用于恢复微信更新数据库时 daemon 恰好启动所造成的联系人加载失败。

### 语音原始文件导出

增强版可以直接读取微信媒体数据库中的 `VoiceInfo`，导出 SILK 原始语音：

```bash
# 导出全部语音
wx voices -o ./voice-export --json

# 只导出指定联系人或群聊
wx voices "张三" -o ./voice-export --json

# 按时间范围导出
wx voices -o ./voice-export --since 2025-01-01 --until 2025-12-31 --json

# 分页或覆盖已有文件
wx voices -o ./voice-export --offset 100 -n 200 --overwrite --json
```

首次使用前需运行 `wx init --force`，确保 `all_keys.json` 中包含
`message/media_*.db` 的密钥。

每条语音会生成一份音频和一份证据文件：

```text
voice-export/
├── 一对一聊天/
│   └── 联系人名称/
│       ├── <timestamp>_<local_id>.silk
│       └── <timestamp>_<local_id>.voice.json
├── 群聊/
│   └── 群名称/
│       ├── <timestamp>_<local_id>.silk
│       └── <timestamp>_<local_id>.voice.json
└── _voice_export_summary.json
```

`.voice.json` 保留会话标识、时间、消息 ID、媒体数据库来源、SILK
头校验和文件路径，便于后续 ASR、校对和回写聊天记录。文件名使用
`timestamp + local_id` 作为稳定键。

### wechat-decrypt 工具箱

增强版将 [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt)
的个人微信与通用工具能力接入 Rust 命令。上游参考源码保存在：

```text
vendor\wechat-decrypt
```

`vendor/wechat-decrypt` 是保留的第三方参考实现与 WASM 等资产来源，不是当前原生工作流的配置目录。`toolkit status` 仍报告参考源码与可选 Python 环境；相关路径不写死本机绝对路径，解析使用：

1. `WX_WECHAT_DECRYPT_DIR` / `WX_WECHAT_DECRYPT_PYTHON`
2. 可执行文件旁的相对目录，如 `vendor\wechat-decrypt`
3. 当前工作目录下的 `vendor\wechat-decrypt`
4. Python 未指定时尝试 `.venv\Scripts\python.exe`，最后尝试 `python`

当前工作流使用选中的 wx-cli 账号配置。先查看配置计划；仅显式 `--apply --yes` 才写入，以下命令不扫描密钥或下载模型：

```powershell
$env:WX_CLI_CONFIG = 'C:\trusted-config\config.json'
wx toolkit setup --config-path 'C:\trusted-config\config.json' --db-dir 'D:\xwechat_files\wxid_example\db_storage'
wx toolkit setup --config-path 'C:\trusted-config\config.json' --db-dir 'D:\xwechat_files\wxid_example\db_storage' --apply --yes
wx toolkit setup --check
```

配置向导不替代 `wx init` 的密钥初始化。输出目录须与原库、密钥和缓存分离，不提交私人配置。只有需要指定保留源码位置或 Python 兼容引擎时，才使用下列环境变量；它们不替代 `WX_CLI_CONFIG`：

```powershell
$env:WX_WECHAT_DECRYPT_DIR='D:\tools\wechat-decrypt'
$env:WX_WECHAT_DECRYPT_PYTHON='D:\tools\wechat-decrypt\.venv\Scripts\python.exe'
```

常用命令：

```bash
wx toolkit status --json
wx toolkit run status
wx toolkit decrypt
wx toolkit export-chats
wx toolkit export-sns --contacts "wxid_example"
wx toolkit export-sns --output-dir existing_sns --adopt-existing --no-remote
wx toolkit decode-images --decoded-dir decoded_images
wx toolkit decode-image input.dat output.jpg
wx toolkit batch-decrypt-images input_dir output_dir
wx toolkit voice-to-mp3 input.silk output.mp3
wx toolkit voice-batch --config voice-config.json --contacts "wxid_example"
wx toolkit export-chats-native native_chats --dry-run
wx toolkit export-chats-native native_chats --users "wxid_example"
wx toolkit export-chats-native native_chats --start "2025-01-01" --end "2025-01-31 23:59:59"
wx toolkit export-chats-native native_chats --incremental
wx toolkit export-sns-native source/sns.db native_sns --contact-db source/contact.db --utc-offset +08:00
wx toolkit export-sns-native source/sns.db native_sns_download --download-media
wx toolkit export-sns-native source/sns.db native_sns_updates --update
wx toolkit decrypt-enterprise source/work.db output/work.db --key-file raw-key.txt
wx toolkit enterprise work_snapshot conversations
wx toolkit enterprise work_snapshot messages --conversations "R:example" --limit 100
wx toolkit enterprise work_snapshot export "R:example" exported/work.json --format json
wx toolkit transcribe-chat exported_chat.json transcribed_chat.json
wx toolkit web
wx toolkit gui
```

#### SILK 转 MP3

`wx toolkit voice-to-mp3` 使用内置 SILK SDK 原生解码，不启动 Python。该命令转换单个微信 SILK 文件：

```bash
# 指定输出文件
wx toolkit voice-to-mp3 input.silk output.mp3

# 省略输出路径时，在输入文件旁生成同名 .mp3
wx toolkit voice-to-mp3 input.silk
```

转换时会移除微信语音可能携带的 `0x02` 前缀，校验 `#!SILK_V3`
文件头及封包边界并补齐结束标记；随后原生解码为 24 kHz、单声道、
16-bit PCM，再调用 `ffmpeg` 编码为 MP3。

运行前确保 `ffmpeg` 已加入 `PATH`；不需要安装 Python 或 `pilk`：

```powershell
ffmpeg -version
```

转换会拒绝覆盖源文件及其硬链接，先编码到临时文件，成功后原子替换目标；当前封包上限为 6000、输入上限为 16 MiB。

#### 原生导出与本地工作台

`export-chats` 已由统一 Rust 宿主处理日期、增量、计划选择、delta 和可选自动转录。旧参数放在 `--` 后；`--with-transcriptions` 可在前面指定。计划与 `--dry-run` 不运行模型。

```powershell
wx toolkit export-chats C:\exports\chats -- --dry-run --users wxid_example
wx toolkit export-chats C:\exports\chats -- --incremental --start 2026-09-01
wx toolkit export-chats C:\exports\chats -- --write-plan-csv C:\exports\plan.csv
wx toolkit export-chats C:\exports\chats -- --from-plan-csv C:\exports\plan.csv --plan-mode whitelist
wx toolkit export-chats C:\exports\chats --with-transcriptions -- --explicit-backend --whisper-binary C:\tools\whisper-cli.exe --whisper-model C:\models\ggml-model.bin
wx toolkit export-messages --contacts wxid_example --output-dir C:\exports\directory --formats csv,html,json --dry-run
wx toolkit export-messages --contacts wxid_example --output-dir C:\exports\directory --formats csv,html,json
wx toolkit web --open
wx toolkit gui
wx toolkit monitor --help
wx toolkit latency --help
wx toolkit cleanup --help
```

`export-messages` 按实际消息表目录导出 CSV/HTML/JSON、`.info` 与媒体目录，不仅依赖最近会话；未映射身份以 `unknown_<完整表哈希>` 保留，不能据此猜测真实联系人。默认拒绝覆盖未知目录，`--update` 只更新已绑定的输出。媒体缺失仍生成明确诊断，默认返回非零；`--allow-missing-media` 可接受有诊断的部分媒体结果。HTML 尝试内嵌本轮已准备的图片，单图 16 MiB、每页图片 URI 合计 64 MiB；超限回退相对引用，音视频及下载链接仍依赖同目录文件。

Web 仅监听 `127.0.0.1`，默认随机空闲端口；`--port` 可指定端口，`--open` 自动打开浏览器。GUI 与 Web 共用服务。服务固定启动账号，支持联系人与历史目录、任务日志、取消、持续消息、通知、自动图片和结构化消息；浏览器不能切换到任意本机账号路径。自动图片使用该账号已有密钥，不隐式扫描或下载；通知仍受浏览器权限约束，刷新重连不会把旧消息作为新通知重放。关闭终端服务后页面不能继续请求，重新选择账号须重启服务。

`toolkit run` 已映射原生工作流，省略子命令启动 Web；不再把未知命令交给 Python 脚本。`run all` 的阶段组合与 `export-chats --with-transcriptions` 不等价，不能因名称为 all 就假定会自动运行 ASR。

`export-chats-native` 按会话表精确 username 导出，不使用最近会话分页；通过 `_export_index.json` 保持联系人文件归属，同名联系人不互相覆盖。支持 `--users`、`WECHAT_EXPORT_USERS` 和不写文件的 `--dry-run`。`--start` / `--end` 接受本地日期、日期时间或 Unix 秒，包含两个端点；仅日期表示当天零点。当前在读取全部消息后筛选，尚未优化数据库读取量。

`--incremental` 保留旧消息、转录及自定义字段，按 `source + local_id` 追加去重，并刷新首末消息日期与当前联系人元数据；日期条件只约束本次追加，不删除旧消息。索引中的旧文件缺失、损坏、身份不符，或旧消息缺少来源而发生同号碰撞时，该会话拒绝写入并明确失败，不猜测分片身份。导出包含旧版四个联系人字段，缺列等兼容回退记录在 `metadata_warnings`。这个独立 `export-chats-native` 入口不串联转录；需要计划、delta 与自动转录组合时使用上面的 `export-chats`。

```powershell
wx toolkit export-delta-native C:\exports\delta-new --users wxid_example --start 2026-09-01 --end 2026-09-07
wx toolkit chat-plan-native --decrypted-dir C:\snapshots\decrypted --message-db message/message_0.db --user wxid_example --output C:\exports\plan.csv
```

`export-delta-native` 固定批次开始时的账号上下文，经 IPC 读取原始消息，按稳定分片名和原始正文计算 `msg_uid`；压缩字节不替换成展示摘要。输出到 `deltas/<run-id>/chats`，最后写入 `manifest.json`；空窗口记录为跳过，分片读取失败不发布部分会话，并返回非零状态。默认要求新输出根目录、父目录已存在；显式 `--append-run` 可在已有普通根目录中创建全新批次，但不复用或覆盖同名 run。`--run-id` 可明确命名批次。未给 `--users` 时使用 `WECHAT_EXPORT_USERS` 或全部会话，时间规则同原生批量导出。输出不能位于原库、配置的解密目录或账号运行目录内。

`chat-plan-native` 输出完整 12 列 CSV，仅使用显式数据库清单和目录，不自动发现账号。可重复 `--message-db`、`--media-db`，并指定 `--resource-db`；元数据可用 `--chats-json` 提供，缺省时显示名回退为 username。默认估算，实际扫描使用 `--size-mode scan --source-dir <账号源目录>` 或 `--media-dir <媒体目录>`，扫描线程数为 1–6。数据库缺失或部分统计会在状态列保留，不冒充完整统计；拒绝覆盖输出。

原生 MCP 迁移入口为 `wx mcp`。使用前显式设置 `$env:WX_CLI_CONFIG='C:\path\config.json'`；初始化和工具列表不读取账号、凭证或模型，也不创建输出与临时目录，首次调用时固定账号配置。当前 17 个工具均已有源码接线，包括 `decode_image`、`decode_voice`、`transcribe_voice`，**不等于旧 17 个工具的全部语义已经迁移**。标准输出只含 JSON-RPC；MCP 输入、输出默认各限 1 MiB，`--max-frame-bytes` 最多 16 MiB。语音准备响应单独使用内部 24 MiB IPC 上限，不放宽公开 MCP 帧或其他查询。同步入口尚不能处理调用期间到达的取消通知，详见 [MCP 协议与缺口](src/mcp/PROTOCOL.md)。

图片解码由宿主启动参数授权本地写出，不是只读工具：

```powershell
# 输出目录必须预先存在；应与账号源、缓存、配置和密钥文件分离。
$env:WX_CLI_CONFIG = 'C:\trusted-config\config.json'
wx mcp --media-output-root 'C:\trusted-output\images' --image-key-file 'C:\trusted-keys\image.json'
```

`--media-output-root` 不自动创建；未设置时图片调用在账号访问前拒绝。`--image-key-file` 可选，但只能由宿主提供，且要求同时设置输出根；V2 需要有效的显式 AES 密钥，不扫描进程、读取自动密钥提供器或下载密钥。MCP 工具参数仅接受 `chat_name`、正数 `local_id` 和可选 `create_time`（默认 0）；不能传入目录、密钥、覆盖或上传选项。

`decode_image` 核验唯一消息及当前账号资源后，将图片以内容摘要命名并无覆盖发布；已有文件也不会被视为可自动复用的成功结果。不下载、不上传、不调用外部图片转换器。**文件发布后，响应仍可能因超限、超时或传输失败而失败；已发布文件不回滚，不能根据错误响应判断“没有输出”。不承诺自动重试、幂等成功或恰好一次执行**，重试前由宿主核对输出目录。

语音工具参数仅为 `chat_name` 和正数媒体 `local_id`，不是消息分片的 local_id。daemon 只准备有界 SILK 与关联证据；host 验证后执行解码或显式 ASR。`decode_voice` 复用预存 `--media-output-root`，按 WAV 的 SHA-256 命名并禁止覆盖；发布前以真实请求 ID 和实际 JSON-RPC 包装执行 `check_text_result`。成功为旧式纯文本模板，不返回内部音频对象。预算预检不能回滚随后发生的通道断开，发布与送达仍非事务。

```powershell
# 本地模型与程序必须显式提供；不自动下载或回退云端。
wx mcp --whisper-binary 'C:\tools\whisper-cli.exe' --whisper-model 'C:\models\ggml-model.bin' --voice-cache-file 'C:\trusted-cache\voices.json'
```

`transcribe_voice` 默认本地后端；未指定 `--temp-root` 时仅在调用 prepare 阶段创建请求私有 `TempDir`，正常结束后清理。云端必须显式选择 `--backend explicit-open-ai`，并提供 `--allow-upload`、端点、模型和凭证文件；工具参数不能选择后端或授权上传。实际后端期限取 IPC 之后的剩余调用预算与配置超时的较小值。`--voice-cache-file` 可选，省略不持久化转录缓存；缓存使用绑定账号身份及消息、音频、识别配置身份，执行 timeout 不再参与键。旧含 timeout 的摘要记录保留，但不会立即命中新摘要。缓存提交前已接入 exact 响应预算、原始守卫、context 与账号检查：预检或最终 persist 前回调拒绝时不发布本次缓存。普通缓存 I/O 故障仍可保留成功识别结果；已提交结果不与 MCP 通道送达组成事务。

持久 receipt 已接入：精确 username、媒体 ID、绑定账号和后端身份匹配时，转录可在源语音删除、daemon 停止后只读返回历史成功缓存；显示名可能仍需在线 ResolveChat。receipt 不是签名，不证明源消息当前存在；冲突或缺失不会冒充成功。`get_chat_images` 已查询并投影 md5/size、resource_status、size_status、size_kind、binding；缺失与歧义显式返回，size 表示加密 DAT 元数据大小，不是明文图片大小。

无源弱缓存不能补造 receipt 身份；有源强身份成功缓存可以在真实 identity/evidence 核验后补索引。MCP 具体边界见 [协议与合成回归](src/mcp/PROTOCOL.md)，当前全量结果统一见 [迁移验收记录](docs/rust-migration.md)。17 项工具接线不等于真实模型质量、用户私有账号或发布验收已验证；[旧工作流缺口审计](docs/legacy-workflow-gap-audit.md)保留历史发现并注明当前去向，不是当前未完成事项列表。

`decode_file_message` 与 `decode_record_item` 只查当前账号的本地缓存，返回元数据及 `found/missing/text/metadata_only` 状态，不下载、写入或解码媒体。转发项索引从零开始；消息与候选文件歧义明确拒绝。有消息 MD5 时核对候选字节，没有时只接受单个候选并返回弱匹配警告；路径不是永久锁或不可变副本。

`voice-batch --config voice-config.json` 只读取显式配置，相对配置路径以该文件所在目录为基准。读取 `decrypted_dir/message/media_0.db` 及联系人库，`output_base_dir` 指定输出目录；也可用 `--output-dir` 覆盖。按联系人写入 `.info` 和 `voice/*.mp3`，同名联系人隔离，已有 MP3 跳过。需要 ffmpeg，不需要 Python；损坏语音按条报告并返回非零状态，不含 ASR。联系人筛选使用 `--contacts` 或 `WECHAT_EXPORT_CONTACTS` 的精确 username。

`toolkit export-sns` 只读取选中账号配置及已解密库，不启动 Python。默认输出到配置旁的 `wechat_files/<账号目录名>/<联系人>/SNS`，可用 `--output-dir` 覆盖；配置中的旧派生项 `output_base_dir` 不覆盖此规则。按 `--contacts` 或 `WECHAT_EXPORT_CONTACTS` 精确筛选数据库 user_name；默认恢复本账号可用缓存，媒体与帖子 JSON 同级，保持旧时间线布局。`--download-media` 或 `WECHAT_SNS_DOWNLOAD_MEDIA=1` 授权远程下载，`--no-remote` 优先禁止网络。缺少 SNS 库或查询结果为空时不改动时间线输出。

时间线更新按本轮查询重写汇总，不把旧帖子重新合并进来；未涉及的旧 JSON、媒体及其他联系人文件保留。新目录绑定账号及联系人，后续自动更新；非空无绑定目录须显式 `--adopt-existing`，冲突绑定不能接管，认领后持续标记 `legacy_unverified`。所有联系人先完成目标预检，再生成内容。媒体、单帖、汇总、HTML、恢复报告按顺序逐文件原子发布，不是跨联系人事务；中途失败可能部分提交，绑定文件不证明导出完成。只引用本轮成功恢复或下载的媒体，不将历史同名文件视为本轮命中。

`export-sns-native` 使用显式已解密库路径，输出同样的单条 JSON、时间线 JSON 和离线 HTML。默认不覆盖已有 `SNS` 目录；从首次导出开始加 `--update` 可绑定数据库路径并持续更新，已有无绑定目录需 `--update --adopt-existing`。默认只保存远程媒体引用；可指定 `--xwechat-cache` 或 `--sns-cache` 恢复本地图片及已有 MP4，缓存媒体保持 `images/`、`videos/` 布局。V2 图片密钥通过 `--image-key-file` 读取，XOR 默认 0x88，可用 `--image-xor-key` 指定。图片匹配是启发式，不能证明媒体身份；视频需 XML 媒体 ID，默认不接受部分视频。仅 `--download-media` 授权下载，环境变量不会隐式开启此入口的网络。恢复状态保存在 `_media_recovery.json`，JSON 与 HTML 使用相同本地引用。失败保留成功项并返回非零退出码。独立相册编排使用 `sns-album`，两者不能共用同一个绑定目录。

`decrypt-enterprise` 仅支持 4096 字节页的离线 wxSQLite3 AES128 主库；`--key-file` 为包含 32 位十六进制原始密钥的 UTF-8 文本文件。拒绝旁路日志和已有输出，不合并 WAL；该格式没有 MAC，SQLite 完整性检查不是密码学认证。目标文件系统须支持硬链接。

原生视频与转录迁移入口：

```powershell
wx toolkit decode-sns-video encrypted.bin video.mp4 --key-file video-key.txt
wx toolkit transcribe-audio-native voice.silk --whisper-binary C:\tools\whisper-cli.exe --whisper-model C:\models\ggml-model.bin
wx toolkit transcribe-chat-native chat.json transcribed.json --media-manifest media.json --media-root C:\audio --whisper-binary C:\tools\whisper-cli.exe --whisper-model C:\models\ggml-model.bin
wx toolkit transcribe-database-native --decrypted-dir C:\snapshots\decrypted --username wxid_example --source message/message_0.db --local-id 7 --whisper-binary C:\tools\whisper-cli.exe --whisper-model C:\models\ggml-model.bin
```

`decode-sns-video` 内嵌已校验哈希的 WASM，以 Rust wasmi 离线运行，不启动 Node。只解码前 128 KiB，其余部分流式复制；拒绝已有输出，明文 MP4 不需要密钥。只验证 `ftyp` 文件头，不保证完整可播放性。`sns-album` 现已复用同一 Rust WASM 核心完成网络与缓存视频编排。

上述 `*-native` ASR 入口默认要求显式本地 whisper.cpp 程序和模型，不下载模型；接受 SILK 或 PCM16 单声道 WAV。其云端调用必须使用 `--backend explicit-open-ai --allow-upload --openai-base-url https://example.com/v1 --openai-model MODEL --api-key-file key.txt`，不从环境读取凭证、不使用系统代理、不跟随重定向。真实模型及 GPU 尚未验证。

`toolkit transcribe-chat` 和 `export-chats --with-transcriptions` 则按固定账号配置自动关联数据库语音。未使用 `--explicit-backend` 时读取 `transcription_backend`，缺省 `local` 对应 Python Whisper/PyTorch，模型名缺省 `base`，可能下载权重；`whisper_cpp` 对应配置的本地程序与模型。云端配置仍须上传授权，凭据读取规则与独立 native 入口不同，见 [本地后端](src/toolkit/asr/LOCAL.md) 与 [云后端](src/toolkit/asr/OPENAI.md)。不因某一后端失败自动换成另一个后端。

MCP 需要兼容配置的 Python 模型时，由宿主使用 `wx mcp --configured-local-python`，配置必须明确为 `transcription_backend="local"`；该开关与 whisper.cpp 路径及云参数互斥，工具请求不能选择或授权引擎。

聊天转录清单为 `{"entries":[{"username":"u","source":"message_0.db","local_id":1,"audio":"voice/1.silk"}]}`，音频路径相对 `--media-root`，完整身份必须与聊天 JSON 匹配。已有转录保留，单条失败报告并继续，批次完成才回写；该聊天命令不自动关联数据库，也不提供逐条持久化或转录缓存。

`transcribe-database-native` 另行支持显式静态快照中的单条语音：在指定消息分片和联系人表定位后，以 username、服务端消息 ID 和时间关联媒体分片，不用同号 local_id 猜测。要求完整证据列及唯一匹配，拒绝活动 WAL/SHM/日志和冲突数据；原始 SILK 直接进入字节转录 API，不创建临时 SILK。结果包含消息侧、媒体侧证据，并标记 `account_authenticated=false`：裸快照的账号来源必须由调用方保证，不能用此标记替代账号认证。批量与 MCP 已有独立 Rust 宿主，使用各自的账号绑定和语音身份契约，不是调用本条 CLI 来冒充整体工作流。

单条数据库转录可成对指定 `--cache-file C:\trusted-output\asr-cache.json --cache-account ACCOUNT` 启用缓存，默认不缓存。父目录须已存在且由调用方控制，缓存不能位于源快照内，也不能与模型、程序或密钥文件重合。账号标记由调用方负责提供，不能证明快照归属。缓存身份包含消息、音频和后端配置，本地模型及程序按内容识别；云模型别名可能在服务端变化，同名不保证模型内容不变。云端即使命中缓存也要求显式授权。缓存读取或写入失败通过结果状态报告，不抹去成功转录；不提供多写入者事务或比较交换保证。

回写期间会锁定协作输出并在发布前复核原文件；请勿同时用其他程序改写该路径。此保护不是针对任意外部写入的严格原子比较替换，边界见 [回写说明](src/toolkit/asr/WRITEBACK.md)。

`enterprise <snapshot>` 读取已解密离线目录中的 `message.db`，可选 `user.db` 和 `session.db`；提供 `contacts`、`conversations`、`messages`、`export` 子命令。可用 `--self-id` 明确本人企业账号 ID，不指定时不猜测本人。消息查询支持会话、发送者、类型、正文包含、分页和时间筛选；时间使用库中原始单位，含起点不含终点。单会话导出支持 JSON、CSV、HTML，父目录须已存在且位于源目录外，禁止覆盖。CSV 文本防公式注入、HTML 转义并禁止联网，不读取个人微信配置。

---

## 架构

```
查询 CLI / MCP -> 查询管道 -> daemon/query_state -> DBCache、联系人缓存
wx tasks / Web -> 认证任务管道 -> daemon/tasks -> 类型化工作进程 -> 领域模块
```

daemon 首次解密后将数据库和 mtime 持久化到当前运行身份的账号目录。重启后 mtime 未变则直接复用，无需重解密；不同账号、配置及运行根目录不会共用后台。

查询缓存按需初始化，缺查询密钥不阻止任务服务启动；队列、取消和历史归 daemon 所有，Web 只维护展示投影。详见[当前架构](docs/architecture.md)。

```
~/.wx-cli/
├── config.json       # 配置
├── all_keys.json     # 数据库密钥
└── accounts/
    └── <runtime-id>/
        ├── daemon.pid / daemon.log
        └── cache/
            ├── _mtimes.json  # mtime 索引
            └── *.db         # 解密后的数据库
```

---

## 原理

微信 4.x 使用 SQLCipher 4 加密本地数据库（AES-256-CBC + HMAC-SHA512，PBKDF2 256,000 次迭代）。WCDB 在进程内存中缓存派生后的 raw key，格式为 `x'<64hex_key><32hex_salt>'`。

Windows 根据 `Weixin.exe` 文件版本选择密钥提供器：

- **微信 4.1.9 及更早版本**：使用 legacy raw-key provider，通过 `VirtualQueryEx` + `ReadProcessMemory` 扫描 `x'<key><salt>'` 候选，并使用数据库 salt 逐一验证。
- **微信 4.1.10 及更新版本**：使用 `Config.Cipher` provider，从多个微信进程只读提取候选并验证；验证不完整时停止更新，不使用旧版扫描方式回退，也不会覆盖已有 `all_keys.json`。

成功提取后，daemon 按需解密数据库并缓存结果。

### Windows 兼容性验证

2026-08-18 使用 `wx 0.3.0-leyan.2` 对 Windows 微信 `4.1.9.57` 完成实机回归测试：

| 检查项 | 结果 |
| --- | --- |
| 版本识别与 provider 分流 | 正确识别 `4.1.9.57`，选择 legacy raw-key provider |
| 加密数据库 | 发现 27 个 |
| 内存候选 | 发现 38 个 |
| 密钥匹配 | 27 个数据库全部匹配 |
| 新旧结果一致性 | 原有 26 条密钥全部一致，无变更、无丢失 |
| 新发现数据库 | `migrate/unspportmsg.db` 1 个 |
| 输出完整性 | 27 条密钥格式均有效，27 个对应数据库文件均存在 |
| 解密读取 | `wx sessions` 与 `wx contacts` 均可正常读取 |

该测试确认微信 `4.1.9.x` 的 legacy 密钥导出路径在引入 `4.1.10+` provider 后没有发生兼容性回归。密钥值、账号信息和本机数据路径未写入测试记录。

---

## 致谢

本项目受 [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) 启发，在其基础上进行了重新设计与实现。感谢原作者的研究与探索。

---

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不得用于未经授权的数据访问。
## 位置消息详细解码

```powershell
wx decode-location "聊天对象" 123 1700000000
wx decode-location "聊天对象" 123 1700000000 --json
```

时间戳可省略；跨分片出现相同消息 ID 时必须提供时间戳，仍不唯一则返回歧义错误。文本展示地点、地址、电话、营业时间和经纬度等；JSON 保留完整结构化位置字段。此命令由 Rust 实现，不启动 Python。更新源代码后需重新构建，既有安装版不会自动更新。
