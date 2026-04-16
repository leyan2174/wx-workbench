<div align="center">

# wx-cli

**从命令行查询本地微信数据**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)](#安装)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)

会话 · 聊天记录 · 搜索 · 联系人 · 群成员 · 收藏 · 统计 · 导出

</div>

---

## 特性

- **零依赖安装** — 单一 Rust 二进制，一行命令装完
- **毫秒级响应** — 后台 daemon 持久缓存解密数据库，mtime 不变则复用
- **AI 友好** — `--json` 输出纯 JSON，方便 LLM agent 直接调用
- **完全本地** — 数据不出本机，实时解密，无需全量预解密

---

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/jackwener/wx-cli/main/install.sh | bash
```

> 支持：macOS Apple Silicon / Intel，Linux x86_64 / arm64

<details>
<summary>其他安装方式</summary>

**手动下载**

从 [Releases](https://github.com/jackwener/wx-cli/releases) 下载对应平台文件，重命名为 `wx`，`chmod +x` 后放入 PATH：

| 平台 | 文件 |
|------|------|
| macOS Apple Silicon | `wx-macos-arm64` |
| macOS Intel | `wx-macos-x86_64` |
| Linux x86_64 | `wx-linux-x86_64` |

**从源码构建**

```bash
git clone git@github.com:jackwener/wx-cli.git && cd wx-cli
cargo build --release
# 产物：target/release/wx
```

</details>

---

## 快速开始

微信 4.x（macOS 版）需要先做 ad-hoc 签名才能扫描内存：

```bash
sudo codesign --force --deep --sign - /Applications/WeChat.app
```

保持微信运行，然后初始化（只需一次）：

```bash
sudo wx init
```

之后直接用，daemon 会在首次调用时自动启动：

```bash
wx sessions        # 查看最近会话
wx history "张三"  # 查看聊天记录
wx search "Claude" # 搜索消息
```

---

## 命令

### 消息

```bash
wx sessions                                      # 最近 20 个会话
wx unread                                        # 有未读消息的会话
wx new-messages                                  # 上次检查后的新消息（增量）
wx history "张三"                                # 最近 50 条记录
wx history "AI群" --since 2026-04-01 --until 2026-04-15
wx search "关键词"                               # 全库搜索
wx search "会议" --in "工作群" --since 2026-01-01
```

### 联系人 & 群组

```bash
wx contacts                  # 联系人列表
wx contacts -q "李"          # 按名字搜索
wx members "AI交流群"        # 群成员列表
```

### 收藏 & 统计

```bash
wx favorites                          # 全部收藏
wx favorites --type image             # 按类型筛选（text/image/article/card/video）
wx favorites -q "关键词"              # 搜索收藏内容
wx stats "AI群"                       # 聊天统计
wx stats "AI群" --since 2026-01-01   # 指定时间范围
```

### 导出

```bash
wx export "张三" --format markdown -o chat.md
wx export "AI群" --since 2026-01-01 --format json
```

### JSON 输出（AI agent 用）

所有命令加 `--json` 输出机器可读的 JSON：

```bash
wx sessions --json
wx search "关键词" --json | jq '.[0].content'
wx new-messages --json
```

### Daemon 管理

```bash
wx daemon status
wx daemon stop
wx daemon logs --follow
```

---

## 架构

```
wx (CLI) ──Unix socket──▶ wx-daemon (后台进程)
                              │
                    ┌─────────┴──────────┐
               DBCache               联系人缓存
           (mtime 感知复用)
```

daemon 首次解密后将数据库和 mtime 持久化到 `~/.wx-cli/cache/`。重启后 mtime 未变则直接复用，无需重解密。

```
~/.wx-cli/
├── config.json       # 配置
├── all_keys.json     # 数据库密钥
├── daemon.sock       # Unix socket
├── daemon.pid / .log
└── cache/
    ├── _mtimes.json  # mtime 索引
    └── *.db          # 解密后的数据库
```

---

## 原理

微信 4.x 使用 SQLCipher 4 加密本地数据库（AES-256-CBC + HMAC-SHA512，PBKDF2 256,000 次迭代）。WCDB 在进程内存中缓存派生后的 raw key，格式为 `x'<64hex_key><32hex_salt>'`。

wx-cli 通过 macOS Mach VM API（`mach_vm_region` + `mach_vm_read`）或 Linux `/proc/<pid>/mem` 扫描微信进程内存，匹配该模式提取密钥，daemon 按需解密并缓存。

---

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不得用于未经授权的数据访问。
