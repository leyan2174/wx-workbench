# wx-cli

微信 4.x (macOS) 本地数据 CLI 工具。从运行中的微信进程内存提取加密密钥，后台常驻 daemon 持久缓存解密数据库，CLI 毫秒级响应。

## 架构

```
wx (CLI) ──Unix socket──▶ wx-daemon (后台进程)
                              │
                    ┌─────────┼─────────┐
               DBCache     联系人缓存   WAL 监听
            (mtime 感知)              (500ms polling)
```

- **wx-daemon**：后台常驻，持有解密后的 DB 热缓存，首次解密后跨重启复用（mtime 不变则不重解密）
- **wx (CLI)**：发 JSON 请求到 Unix socket，获得响应后格式化输出；首次调用自动启动 daemon

## 快速开始

### 环境要求

- macOS (Apple Silicon / Intel)
- WeChat 4.x (macOS 版，需 ad-hoc 签名，见下文)
- Python 3.12+
- [uv](https://docs.astral.sh/uv/)（Python 包管理）
- Xcode Command Line Tools：`xcode-select --install`

### 安装

```bash
git clone git@github.com:jackwener/wx-cli.git
cd wx-cli
uv sync
```

### 初始化（首次使用）

打开微信并登录，运行初始化：

```bash
uv run python wx.py init
```

`wx init` 自动完成：
1. 检测微信数据目录（`~/Library/Containers/.../xwechat_files/<wxid>/db_storage`）
2. 编译 C 内存扫描器（如未编译）
3. `sudo` 扫描微信进程内存，提取所有数据库密钥 → `all_keys.json`
4. 更新 `config.json`

### 使用

```bash
# 最近会话
uv run python wx.py sessions

# 聊天记录
uv run python wx.py history "张三"
uv run python wx.py history "AI群" --since 2026-04-01 --until 2026-04-15

# 搜索消息
uv run python wx.py search "Claude"
uv run python wx.py search "会议" --in "工作群" --since 2026-01-01

# 联系人
uv run python wx.py contacts
uv run python wx.py contacts -q "李"

# 导出聊天记录
uv run python wx.py export "张三" --format markdown -o chat.md
uv run python wx.py export "AI群" --since 2026-01-01 --format json -o chat.json

# 实时监听新消息（Ctrl+C 退出）
uv run python wx.py watch
uv run python wx.py watch --chat "AI交流群"
uv run python wx.py watch --json | jq .content

# daemon 管理
uv run python wx.py daemon status
uv run python wx.py daemon stop
uv run python wx.py daemon logs
uv run python wx.py daemon logs --follow
```

> **注**：daemon 在首次 CLI 调用时自动启动，无需手动运行。

### 可选：设置别名

```bash
echo 'alias wx="uv run --directory /path/to/wx-cli python wx.py"' >> ~/.zshrc
source ~/.zshrc

# 之后可以直接用
wx sessions
wx history "张三"
wx watch
```

## 命令参考

### `wx init [--force]`
首次初始化：检测数据目录、编译扫描器、提取密钥、写入配置。`--force` 强制重新扫描（微信更新后使用）。

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
管理后台 daemon。`logs --follow` 等同 `tail -f`。

> **如果 `wx init` 报权限错误**（`task_for_pid` 失败），说明你的微信版本开启了更严格的 hardened runtime 保护，需要先剥掉签名：
> ```bash
> sudo codesign --force --deep --sign - /Applications/WeChat.app
> ```
> 官网 dmg 安装的微信通常**不需要**这步。

## 原理

### 密钥提取

微信 4.x 使用 SQLCipher 4 加密本地数据库：
- **加密**：AES-256-CBC + HMAC-SHA512
- **KDF**：PBKDF2-HMAC-SHA512，256,000 次迭代
- **页结构**：4096 bytes/page，reserve = 80（IV 16 + HMAC 64）

WCDB 在进程内存中缓存派生后的 raw key，格式为 `x'<64hex_enc_key><32hex_salt>'`。C 扫描器（`find_all_keys_macos.c`）通过 macOS Mach VM API 扫描微信进程内存，匹配此模式，再用 HMAC 校验 page 1 确认密钥正确性，输出到 `all_keys.json`。

### DBCache（mtime 感知缓存）

daemon 首次解密后将结果（及 DB/WAL 的 mtime）持久化到 `~/.wechat-cli/cache/_mtimes.json`。重启时若 mtime 未变，直接复用已解密文件，无需重新解密。

### WAL 监听

微信使用 SQLite WAL 模式（WAL 文件固定预分配 4MB，不能靠文件大小判断变化）。daemon 每 500ms 检测 `session.db-wal` 的 mtime，有变化时重新解密并广播新消息给所有 `watch` 客户端。

### 数据文件路径

```
~/.wechat-cli/
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
| `media_*/media_*.db` | 媒体文件索引 |

## 测试

```bash
uv run python -m pytest tests/ -v
```

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不要用于未经授权的数据访问。
