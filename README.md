# wx-cli

微信 4.x (macOS/Linux/Windows) 本地数据 CLI 工具。从运行中的微信进程内存提取加密密钥，后台常驻 daemon 持久缓存解密数据库，CLI 毫秒级响应。

单一 Rust 二进制，无运行时依赖。

## 架构

```
wx (CLI) ──Unix socket──▶ wx-daemon (后台进程)
                              │
                    ┌─────────┼─────────┐
               DBCache     联系人缓存   WAL 监听
            (mtime 感知)              (500ms polling)
```

- **wx-daemon**：后台常驻，持有解密后的 DB 热缓存，mtime 不变则跨重启复用，无需重解密
- **wx (CLI)**：发 JSON 请求到 Unix socket，格式化输出；首次调用自动启动 daemon

## 快速开始

### 环境要求

- macOS（Apple Silicon / Intel）或 Linux
- WeChat 4.x（macOS 版）
- 首次使用需 `sudo`（内存扫描）

### 安装

**方式一：cargo install（推荐）**

```bash
cargo install wx-cli
```

**方式二：下载预编译二进制**

从 [Releases](https://github.com/jackwener/wx-cli/releases) 下载对应平台的文件：

| 平台 | 文件名 |
|------|--------|
| macOS Apple Silicon | `wx-macos-arm64` |
| macOS Intel | `wx-macos-x86_64` |
| Linux x86_64 | `wx-linux-x86_64` |
| Windows x86_64 | `wx-windows-x86_64.exe` |

```bash
# macOS arm64 示例
curl -L https://github.com/jackwener/wx-cli/releases/latest/download/wx-macos-arm64 -o wx
chmod +x wx
sudo mv wx /usr/local/bin/
```

**方式三：从源码构建**

```bash
git clone git@github.com:jackwener/wx-cli.git
cd wx-cli
cargo build --release
# 二进制位于 target/release/wx
```

### 初始化（首次使用）

macOS 上微信需要 ad-hoc 签名才能被扫描内存：

```bash
sudo codesign --force --deep --sign - /Applications/WeChat.app
```

打开微信并登录，然后运行初始化：

```bash
sudo wx init
```

`wx init` 自动完成：
1. 检测微信数据目录（`~/Library/Containers/.../xwechat_files/<wxid>/db_storage`）
2. 扫描微信进程内存，提取所有数据库密钥 → `~/.wechat-cli/all_keys.json`
3. 写入 `~/.wechat-cli/config.json`

### 使用

```bash
# 最近会话
wx sessions

# 聊天记录
wx history "张三"
wx history "AI群" --since 2026-04-01 --until 2026-04-15

# 搜索消息
wx search "Claude"
wx search "会议" --in "工作群" --since 2026-01-01

# 联系人
wx contacts
wx contacts -q "李"

# 导出聊天记录
wx export "张三" --format markdown -o chat.md
wx export "AI群" --since 2026-01-01 --format json -o chat.json

# 实时监听新消息（Ctrl+C 退出）
wx watch
wx watch --chat "AI交流群"
wx watch --json | jq .content

# daemon 管理
wx daemon status
wx daemon stop
wx daemon logs
wx daemon logs --follow
```

> daemon 在首次 CLI 调用时自动启动，无需手动运行。

## 命令参考

### `wx init [--force]`
首次初始化：检测数据目录、扫描内存、提取密钥、写入配置。`--force` 强制重新扫描（微信更新后使用）。

### `wx sessions [-n N] [--json]`
列出最近 N 个会话（默认 20），显示未读数、最后消息摘要。

### `wx history CHAT [-n N] [--offset N] [--since DATE] [--until DATE] [--json]`
查看指定聊天的消息记录。`DATE` 格式：`YYYY-MM-DD` 或 `YYYY-MM-DD HH:MM`。

### `wx search KEYWORD [--in CHAT]... [-n N] [--since DATE] [--until DATE] [--json]`
全库搜索消息，`--in` 可指定多个聊天范围。

### `wx contacts [-q QUERY] [-n N] [--json]`
列出或搜索联系人。

### `wx export CHAT [-f FORMAT] [-o FILE] [-n N] [--since DATE] [--until DATE]`
导出聊天记录。`-f` 支持 `markdown`（默认）、`txt`、`json`。`-o` 指定输出文件，不指定则输出到 stdout。

### `wx watch [--chat CHAT] [--json]`
实时监听新消息（WAL 变化推送，约 500ms 延迟）。`--json` 输出 JSON lines，方便 `jq` 处理。

### `wx daemon status / stop / logs [-f] [-n N]`
管理后台 daemon。`logs --follow` 持续追踪新日志。

## 原理

### 密钥提取

微信 4.x 使用 SQLCipher 4 加密本地数据库：
- **加密**：AES-256-CBC + HMAC-SHA512
- **KDF**：PBKDF2-HMAC-SHA512，256,000 次迭代
- **页结构**：4096 bytes/page，reserve = 80（IV 16 + HMAC 64）

WCDB 在进程内存中缓存派生后的 raw key，格式为 `x'<64hex_enc_key><32hex_salt>'`。Rust 扫描器通过 macOS Mach VM API（`mach_vm_region` + `mach_vm_read`）或 Linux `/proc/<pid>/mem` 扫描微信进程内存，匹配此模式后输出到 `~/.wechat-cli/all_keys.json`。

### DBCache（mtime 感知缓存）

daemon 首次解密后将结果（及 DB/WAL 的 mtime，精度纳秒）持久化到 `~/.wechat-cli/cache/_mtimes.json`。重启时若 mtime 未变，直接复用已解密文件。

### WAL 监听

daemon 每 500ms 检测 `session.db-wal` 的 mtime，有变化时重新解密并通过 Unix socket 广播新消息给所有 `watch` 客户端。

### 数据文件路径

```
~/.wechat-cli/
├── config.json       # 配置（DB 目录、密钥文件路径）
├── all_keys.json     # 数据库密钥
├── daemon.sock       # Unix socket
├── daemon.pid        # PID 文件
├── daemon.log        # daemon 日志
└── cache/
    ├── _mtimes.json  # mtime 持久化索引
    └── *.db          # 解密后的数据库缓存
```

## 数据库结构

解密后约 26 个数据库：

| 路径 | 内容 |
|------|------|
| `session/session.db` | 会话列表（最新消息摘要、未读数） |
| `message/message_*.db` | 聊天记录（按 `Msg_<md5(username)>` 分表） |
| `contact/contact.db` | 联系人（username、nick_name、remark） |

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不要用于未经授权的数据访问。
