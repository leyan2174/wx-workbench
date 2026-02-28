# WeChat 4.0 Database Decryptor

微信 4.0 (Windows) 本地数据库解密工具。从运行中的微信进程内存提取加密密钥，解密所有 SQLCipher 4 加密数据库，并提供实时消息监听。

## 原理

微信 4.0 使用 SQLCipher 4 加密本地数据库：
- **加密算法**: AES-256-CBC + HMAC-SHA512
- **KDF**: PBKDF2-HMAC-SHA512, 256,000 iterations
- **页面大小**: 4096 bytes, reserve = 80 (IV 16 + HMAC 64)
- **每个数据库有独立的 salt 和 enc_key**

WCDB (微信的 SQLCipher 封装) 会在进程内存中缓存派生后的 raw key，格式为 `x'<64hex_enc_key><32hex_salt>'`。本工具通过扫描进程内存中的这种模式，匹配数据库文件的 salt，并通过 HMAC 验证来提取正确的密钥。

## 使用方法

### 环境要求

- Windows 10/11
- Python 3.10+
- 微信 4.0 (正在运行)
- 需要管理员权限 (读取进程内存)

### 安装依赖

```bash
pip install pycryptodome
```

### 1. 配置

复制配置模板并修改：

```bash
copy config.example.json config.json
```

编辑 `config.json`：
```json
{
    "db_dir": "D:\\xwechat_files\\你的微信ID\\db_storage",
    "keys_file": "all_keys.json",
    "decrypted_dir": "decrypted",
    "wechat_process": "Weixin.exe"
}
```

`db_dir` 路径可以在 微信设置 → 文件管理 中找到。

### 2. 提取密钥

确保微信正在运行，以**管理员权限**运行：

```bash
python find_all_keys.py
```

密钥将保存到 `all_keys.json`。

### 3. 解密数据库

```bash
python decrypt_db.py
```

解密后的数据库保存在 `decrypted/` 目录，可以直接用 SQLite 工具打开。

### 4. 实时消息监听

#### Web UI (推荐)

```bash
python monitor_web.py
```

打开 http://localhost:5678 查看实时消息流。

- 30ms 轮询 WAL 文件变化 (mtime)
- 检测到变化后全量解密 + WAL patch (~70ms)
- SSE 实时推送到浏览器
- 总延迟约 100ms

#### 命令行

```bash
python monitor.py
```

每 3 秒轮询一次，在终端显示新消息。

## 文件说明

| 文件 | 说明 |
|------|------|
| `config.py` | 配置加载器 |
| `find_all_keys.py` | 从微信进程内存提取所有数据库密钥 |
| `decrypt_db.py` | 全量解密所有数据库 |
| `monitor_web.py` | 实时消息监听 (Web UI + SSE) |
| `monitor.py` | 实时消息监听 (命令行) |
| `latency_test.py` | 延迟测量诊断工具 |

## 技术细节

### WAL 处理

微信使用 SQLite WAL 模式，WAL 文件是**预分配固定大小** (4MB)。检测变化时：
- 不能用文件大小 (永远不变)
- 使用 mtime 检测写入
- 解密 WAL frame 时需校验 salt 值，跳过旧周期遗留的 frame

### 数据库结构

解密后包含约 26 个数据库：
- `session/session.db` - 会话列表 (最新消息摘要)
- `message/message_*.db` - 聊天记录
- `contact/contact.db` - 联系人
- `media_*/media_*.db` - 媒体文件索引
- 其他: head_image, favorite, sns, emoticon 等

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不要用于未经授权的数据访问。
