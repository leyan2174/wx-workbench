# Daemon 任务服务

`wx tasks` 与本地 Web 共用同一账号 daemon 中的任务服务。Web 不再拥有任务队列、工作进程或任务日志文件。普通查询仍走原查询 IPC；任务走带版本和认证的独立命名管道，但由同一 daemon 进程托管。

本轮归并的是已有 Web 的 12 类任务。旧 `toolkit` 直接命令、MCP 的 17 个工具、按消息自动图片解码和企业离线查询并未全部改成任务 RPC，不能把这一阶段写成“所有业务均已迁入 daemon”。

## 命令

先通过 `WX_CLI_CONFIG`、`WX_CLI_HOME` 固定账号配置与运行根；所有示例只表示命令形式，不包含真实账号或凭证。

```powershell
wx tasks info
wx tasks configure
wx tasks submit export-all --users "synthetic-user" --formats json --no-images --wait
wx tasks list
wx tasks get <64位小写十六进制任务ID>
wx tasks logs <任务ID> --follow
wx tasks cancel <任务ID>
```

`submit` 默认立即返回任务 JSON；`--wait` 等待终态，日志写 stderr，最终任务 JSON 写 stdout。失败、取消或中断会返回非零退出码。等待期间 Ctrl+C 只退出客户端，不取消后台任务。任务 ID 在提交前写入 stderr，供不确定结果时查询。

`info`、`list`、`get` 等命令必要时启动所选账号的 daemon。`submit` 仅在未配置时绑定默认设置；已有设置不会被静默覆盖。`configure` 用于绑定企业快照、输入、密钥文件、发现根、允许 PID 或图片缓存等启动设置。设置不一致时返回冲突，需先停止 daemon 再重新配置；浏览器不能通过任务请求传入任意路径。

## 支持的任务

| 任务类型 | 执行内容 | 额外边界 |
| --- | --- | --- |
| `wechat-keys` | 数据库取钥 | 必须逐任务提供 `--authorize-memory-scan` |
| `wechat-decrypt` | 数据库快照解密 | 使用已保存密钥，不隐式扫描 |
| `image-key` | 图片取钥 | 必须逐任务授权，输出日志完全抑制 |
| `export-all` | 聊天导出，可选 SNS、语音及转录 | 筛选和格式显式传递；云转录必须另有上传授权 |
| `decode-images` | 图片批量解码 | 不接受任意输出路径 |
| `sns-decrypt` | SNS 归档、导出 | 媒体下载是显式选项 |
| `voice-mp3` | 数据库语音批量转 MP3 | FFmpeg 依赖不因任务托管而消失 |
| `wxwork-discover` | 保留的企业账号发现 | 发现根在配置阶段固定 |
| `wxwork-scan` | 保留的企业取钥 | 逐任务授权，受固定 Data/PID 限制 |
| `wxwork-decrypt` | 保留的企业单库或批量解密 | 输入模式及密钥路径预先绑定 |
| `wxwork-export` | 保留的企业快照导出 | 选择 `--users` 或显式 `--all-conversations` |
| `wxwork-run` | 保留的企业解密与导出组合 | 文件密钥或显式扫描授权，且明确会话范围 |

CLI 同时接受连字符和下划线形式；JSON 协议使用下划线。企业任务仅维持既有能力，没有扩大企业微信移植范围。完整参数以 `wx tasks submit --help` 和 `wx tasks configure --help` 为准。

## 生命周期与恢复

- 关闭浏览器或 Web 服务，不取消已提交任务，也不停止 daemon。Web 只在启动时确保 daemon 存在，不会在用户执行 `daemon stop` 后自行重启它。
- `wx daemon stop` 请求优雅停机：停止接收新任务、取消队列、回收工作进程及其子孙进程、保存终态。身份匹配的后台无响应时保留有界的强制停止回退。
- 运行任务先创建为挂起进程，加入 Windows Job 后再恢复；取消、正常退出和异常退出均回收 Job。不把已发布文件回滚为“从未执行”。
- daemon 意外退出后，历史中的未完成任务标为 `interrupted`，不自动重放；显式优雅停止的任务标为 `cancelled`。重启后可查询历史，提交新任务前重新绑定启动设置。
- 查询数据库按需初始化。缺密钥或查询初始化失败不阻止任务服务启动；修复密钥后可在同一 daemon 内重试。任务更新密钥后使查询快照失效，失效前等待正在使用该快照的查询退出。

任务状态包括 `queued`、`running`、`cancelling`、`succeeded`、`failed`、`cancelled`、`interrupted`。`cancelling` 不是终态，只有进程回收后才进入 `cancelled`。

## 幂等与容量

通过 `--request-id` 指定 64 位小写十六进制 ID，可查询不确定提交结果或用原参数重试。相同 ID、任务、设置及不可变配置返回同一记录；不同内容复用 ID 会被拒绝。客户端从不自动重发已发送的提交。

HTTP 可用 `Idempotency-Key` 传相同格式的 ID；省略时由 Web 生成。连接断开或超时不等于任务未执行，不能盲目生成新 ID 再提交。

| 限制 | 当前值 |
| --- | --- |
| 等待队列 | 8 项，另有 1 个串行执行槽 |
| 历史 | 100 项，容量满时淘汰最早的终态任务 |
| 每任务日志 | 256 行，每行脱敏后最多 512 字符 |
| 事件缓存 | 最多 256 个事件且不超过 4 MiB 数据预算 |
| 任务 RPC | 请求 64 KiB、回复 8 MiB、总超时 10 秒 |
| 任务管道并发连接 | 32 |
| 查询管道 | 64 个连接，首帧 64 KiB / 5 秒 |

幂等保证限于仍保留的历史记录，不承诺无限期 exactly-once。日志截断带序号缺口；事件游标过期或重启后须按 `reset` 重新读取当前任务列表，不伪装为无遗漏连续日志。

## 存储与认证

任务历史保存为 `WX_CLI_HOME/accounts/<runtime-id>/tasks-history.json`；原 `web-history.json` 仅作只读兼容恢复来源，不覆盖原文件。输出沿用 `WX_CLI_HOME/web-output/<runtime-id>/<task-id>/`，避免破坏已有产物路径约定。

任务命名管道为 `wx-cli-tasks-v1-<runtime-id>`。管道仅授予当前用户访问并拒绝远程客户端；连接后校验实际服务 PID、创建时间、可执行文件及运行身份，再发送令牌。`service-token.key` 在首次写入前设置当前用户私有 ACL，正常停机按已持有文件句柄清理；重启只轮换经身份、单链接和权限核验的陈旧令牌，不覆盖任意同名文件。

此为同 Windows 用户的本地服务边界，不隔离已完全控制该用户账户的恶意代码。Web 的 Host、Origin、token、CSRF 校验继续保留。密钥不会作为公开任务参数或日志输出；内存扫描和云上传仍须显式授权。

## 分层与验证

- `src/service`：类型化协议、参数校验、固定设置、计划、客户端及认证管道。
- `src/daemon/tasks`：任务记录、持久化、队列、取消、日志与进程生命周期。
- `src/cli/tasks.rs` 和 `src/toolkit/web`：命令/HTTP 适配及展示。
- `src/cli/task_worker.rs`：过渡期内部执行桥，按类型调用现有 Rust 处理函数，不解析公开 CLI 参数，也不递归提交任务。
- `src/toolkit`、`src/attachment` 等：已有领域能力。后续可逐项提取执行桥中的 CLI 处理函数；不为这一阶段重写算法。

合成进程验收在 `tests/fixtures/daemon-tasks/runtime.rs`，由 `runtime_isolation` 注册；覆盖导出、幂等、跨账号隔离、设置冲突、日志、重启、Web/CLI 共享、工作进程取消和停机。Job 后代回收、协议边界、私有权限、队列持久化及懒初始化另有单元测试。复跑入口见[测试说明](../tests/README.md)，实际结果见[迁移验收记录](rust-migration.md)。不以这些测试证明真实账号完整性、模型质量或安装部署已验证。
