# Daemon 任务服务

`wx tasks`、本地 Web 与显式启用的 MCP 任务工具共用同一账号 daemon 中的任务服务。入口不拥有任务队列、工作进程或任务日志文件。普通查询保持原有通道；任务走带版本和认证的独立命名管道，但由同一 daemon 进程托管。

提供 个人微信持久任务。其他业务入口也经过 daemon，但不属于持久任务队列：CLI 使用类型化前台操作，MCP 原有查询及同步图片使用认证 `Call::Mcp`，Web 监控、图片与预览使用认证 `Call::Web`。业务归 daemon 所有，不意味着每种操作都进入持久队列，或全部运行于同一个 OS 进程。详见 [业务入口与运行边界](daemon-entrypoints.md)。

## MCP 任务工具

默认 `wx mcp` 保留原有 15 个工具；后台任务默认关闭。宿主在启动参数中授权，工具请求不能添加授权或改路径：

```powershell
# WX_CLI_CONFIG 必须显式指定合成/目标账号；WX_CLI_HOME 与 CLI/Web 保持一致。
wx mcp --tasks --task-kind wechat_decrypt
# 仅管理现有任务（无提交工具）：wx mcp --tasks
# 导出需另授固定目录媒体写入权限：
wx mcp --tasks --task-kind export_all --task-allow-media-write
```

启用 `--tasks` 后增加 `list_tasks`、`get_task`、`cancel_task`、`get_task_events`。只有存在可提交的授权类型时才增加 `submit_task`；类型枚举由共享 `service::plan::capabilities()` 与宿主授权取交集，未实现的类型不会出现。任务类型使用下划线，参数形状及选项约束沿用共享 `Submission` / `Options` 和 `plan::validate`，拒绝未知字段、任意命令、可执行程序、后端及输出路径。

初始化 MCP 后，`tools/call` 示例：

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"submit_task","arguments":{"idempotency_key":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","kind":"wechat_decrypt"}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_tasks","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_task","arguments":{"id":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_task_events","arguments":{"after":0,"limit":32,"wait_ms":0}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"cancel_task","arguments":{"id":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}}
```

示例 ID 只是格式示范；每个新意图应生成新的 64 位小写十六进制幂等键，并在发送前保存。相同意图重试必须保留原键和参数。提交只等待 daemon 接受，不等待任务执行完成；成功结果同时放入 `structuredContent` 与 JSON 文本 `content`，直接保留 daemon 的任务 ID、状态及返回字段。形状错误返回 JSON-RPC `-32602`；业务错误使用 `isError: true` 和 `structuredContent.error.code`，不返回底层异常或配置内容。事件的 `after`、`reset`、日志截断语义不变；`limit` 为 1–128，`wait_ms` 为 0–2000。

### 宿主授权与账号

- `--task-kind` 是任务类型白名单，也明确授权该类型在配置绑定目录内的数据库/密钥写入；不会隐式扫描。`--tasks` 本身允许查看及取消固定账号的全部保留任务，包括 CLI/Web 提交的任务。
- 取钥还需 `--task-allow-memory-scan`，并在提交中设置 `authorize_memory_scan: true`。模型的布尔值不能替代宿主开关。
- 导出、图片和朋友圈任务还需 `--task-allow-media-write`。这是 daemon 固定输出目录的授权，不更改同步图片工具的 `--media-output-root`。组合导出的 `include_sns` 还需允许 `sns_decrypt` 类型。
- 朋友圈媒体下载另需 `--task-allow-media-download`，不由导出授权自动获得。
- `--task-image-cache-dir` 只能由宿主传入，并通过现有 `Configure` 绑定；与当前 daemon 设置不一致会报 `settings_conflict`，不会覆盖。模型没有 `configure` 工具。
- 查询和任务共享同一个惰性固定的 `RuntimeContext`，首次实际访问必须显式配置 `WX_CLI_CONFIG`。启用任务后记录共享配置指纹，每次任务调用前后复核；账号切换或不可变配置变化会使本 MCP 会话失效，需重启，不静默换账号。沿用共享指纹对合法图片密钥轮换的豁免，不另外发明配置身份规则。

### 生命周期与调用链

`MCP tools/call → cli::mcp_tasks → service::client::{wait_ready,request_with_timeout} → Call::{Configure,Submit,List,Get,Cancel,Events} → daemon::tasks → daemon::operations::task_worker`。后台启动仍调用已有 `ensure_running_quiet`；认证、进程身份、请求/回复上限和超时均走现有 service transport。

共享 `Info/Configure` 响应包含只读 `config_fingerprint`（尚未配置时为 null）。MCP 在任务 RPC 前核对后台绑定指纹与本会话指纹，不能通过调用参数覆盖它；缺少或不匹配时拒绝。后台提交时仍由原有 `ConfigPin` 验证配置与绑定一致。

任务 RPC 不依附于 `Call::Mcp` 的查询会话。MCP EOF、宿主退出、取消通知或工具超时都不会额外发送后台取消，也不会把已接受任务转交 MCP 执行。当前 stdio 协议串行处理请求，通知不会抢占正在处理的工具调用；后台取消必须显式调用 `cancel_task`（或 CLI/Web 的取消接口）。取消可能已完成的任务沿用 daemon 的终态幂等语义，文件产物不回滚。

提交响应丢失或超时意味着结果可能未知，不等于未提交：先用原键作为 ID 查询，或用相同键、参数重试。MCP 不自动重发。错误响应、帧超限或断连都不改变 daemon 的既有幂等保留期。daemon 意外退出后的未完成任务仍是 `interrupted`，不是自动断点续跑；优雅停机仍为 `cancelled`。事件游标不承诺跨重启连续。

## 命令

先通过 `WX_CLI_CONFIG`、`WX_CLI_HOME` 固定账号配置与运行根；所有示例只表示命令形式，不包含真实账号或凭证。

```powershell
wx tasks info
wx tasks configure
wx tasks submit export_all --users "synthetic-user" --formats json --no-images --wait
wx tasks list
# 将提交结果中的 id 赋给 $taskId 后，按需执行查询或取消。
wx tasks get $taskId
wx tasks logs $taskId --follow
wx tasks cancel $taskId
```

`submit` 默认立即返回任务 JSON；`--wait` 等待终态，日志写 stderr，最终任务 JSON 写 stdout。失败、取消或中断会返回非零退出码。等待期间 Ctrl+C 只退出客户端，不取消后台任务。任务 ID 在提交前写入 stderr，供不确定结果时查询。

`info`、`list`、`get` 等命令必要时启动所选账号的 daemon。`submit` 仅在未配置时绑定默认设置；已有设置不会被静默覆盖。`configure` 可通过 `--image-cache-dir` 绑定图片缓存目录。设置不一致时返回冲突，需先停止 daemon 再重新配置；浏览器不能通过任务请求传入任意路径。

## 支持的任务

| 任务类型 | 执行内容 | 额外边界 |
| --- | --- | --- |
| `wechat_keys` | 数据库取钥 | 必须逐任务提供 `--authorize-memory-scan` |
| `wechat_decrypt` | 数据库快照解密 | 使用已保存密钥，不隐式扫描 |
| `image_key` | 图片取钥 | 必须逐任务授权，输出日志完全抑制 |
| `export_all` | 聊天导出，可选 SNS | 筛选和格式显式传递；完整聊天保留语音引用 |
| `decode_images` | 图片批量解码 | 不接受任意输出路径 |
| `sns_decrypt` | SNS 归档、导出 | 媒体下载是显式选项 |

CLI 与 JSON 协议只接受下划线形式。旧连字符拼写、企业任务类型、`--enterprise-*` 和 `--all-conversations` 均被拒绝。完整参数以 `wx tasks submit --help` 和 `wx tasks configure --help` 为准。

## 生命周期与恢复

- 关闭浏览器或 Web 服务，不取消已提交任务，也不停止 daemon。Web 只在启动时确保 daemon 存在，不会在用户执行 `daemon stop` 后自行重启它。
- Web 监控循环和游标归 daemon，关闭 HTTP 不停止该监控。其他业务 CLI 的前台操作有独立 15 秒续租，退出或失联会取消并回收，不适用持久任务的“退出等待仍继续”规则。
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
| 任务 RPC | 请求 64 KiB、回复 8 MiB；默认超时 10 秒，MCP 使用现有可配置超时接口并受剩余工具预算约束（默认 30 秒） |
| 任务管道并发连接 | 32 |
| 查询管道 | 64 个连接，首帧 64 KiB / 5 秒 |

幂等保证限于仍保留的历史记录，不承诺无限期 exactly-once。日志截断带序号缺口；事件游标过期或重启后须按 `reset` 重新读取当前任务列表，不伪装为无遗漏连续日志。

## 存储与认证

任务历史保存为 `WX_CLI_HOME/accounts/<runtime-id>/tasks-history.json`；原 `web-history.json` 仅作只读兼容恢复来源，不覆盖原文件。输出沿用 `WX_CLI_HOME/web-output/<runtime-id>/<task-id>/`，避免破坏已有产物路径约定。

升级时，含旧企业任务或已移除字段的历史会先保存为当前账号私有的 `tasks-history-retired-<SHA256>.json` 原始快照，再恢复其中的个人微信任务。企业任务不再列出或执行，原输出目录不删除。个人微信旧配置中的企业字段不再解析或访问；旧版本幂等指纹可能因设置结构变化而冲突，此时应查询旧任务，不要盲目重提。

任务命名管道为 `wx-cli-tasks-v1-<runtime-id>`。管道仅授予当前用户访问并拒绝远程客户端；连接后校验实际服务 PID、创建时间、可执行文件及运行身份，再发送令牌。`service-token.key` 在首次写入前设置当前用户私有 ACL，正常停机按已持有文件句柄清理；重启只轮换经身份、单链接和权限核验的陈旧令牌，不覆盖任意同名文件。

此为同 Windows 用户的本地服务边界，不隔离已完全控制该用户账户的恶意代码。Web 的 Host、Origin、token、CSRF 校验继续保留。密钥不会作为公开任务参数或日志输出；内存扫描和媒体下载仍须显式授权。

## 分层与验证

- `src/service`：类型化协议、参数校验、固定设置、计划、客户端及认证管道。
- `src/daemon/tasks`：任务记录、持久化、队列、取消、日志与进程生命周期。
- `src/cli/tasks.rs` 和 `src/web`：命令/HTTP 适配及展示。
- `src/daemon/operations/task_worker.rs`：内部类型化任务执行桥，调用 daemon 侧业务编排与领域模块，不解析公开 CLI 参数，也不递归提交任务。
- `src/daemon/operation_service.rs`、`operation_worker.rs`：另行监督非持久前台操作的 Job、字节流和取消租约，不复用持久任务日志作为 CLI 输出。
- `src/daemon/mcp_service.rs`、`web_service.rs`：MCP 和 Web 业务状态；HTTP/stdio 入口不维护业务副本。
- `src/business`、`src/application`、`src/adapters/wechat`、`src/infrastructure`：分别提供业务契约、用例编排、微信格式适配和共享执行能力；daemon 或其拥有的 worker 直接调用这些真实所有者。

合成进程测试在 `tests/fixtures/daemon-tasks/runtime.rs`，由 `runtime_isolation` 注册；覆盖导出、幂等、跨账号隔离、设置冲突、日志、重启、Web/CLI 共享、工作进程取消和停机。Job 后代回收、协议边界、私有权限、队列持久化及懒初始化另有单元测试。复跑入口见[测试说明](../tests/README.md)。这些测试不代替真实账号完整性、安装部署验证。
