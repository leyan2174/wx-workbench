# 业务入口与 daemon

CLI、MCP 和 Web 是协议适配层。daemon 持有账号运行状态、业务编排、任务和监督；领域模块提供可复用的查询、媒体和发布能力。

## 入口分工

| 入口 | 传输与职责 | 不应持有的内容 |
| --- | --- | --- |
| 查询 CLI | 参数校验、查询客户端、输出格式。 | 数据库缓存和查询状态副本。 |
| 操作 CLI | 类型化 Operation、前台租约及字节输出。 | 自行执行的媒体/初始化业务进程树。 |
| `wx tasks` | 持久任务提交、查看、取消和等待。 | 另一套任务队列。 |
| MCP | stdio 帧、JSON-RPC、宿主参数封送、任务授权与固定账号指纹核验。 | 长期业务账号锁、任务调度执行、媒体发布。 |
| Web | HTTP、认证、请求与展示。 | 独立监控循环、任务历史与工作进程。 |

业务收口不意味着所有操作都进入持久队列。查询、前台操作、持久任务、MCP 会话和 Web 业务各自维护合适的生命周期。

## 后台身份与启停

`daemon.pid` 是后台身份记录，包含账号运行身份、PID、进程创建时间和可执行文件路径。连接认证、启动前存活检查和停止操作均将该记录限制为 16 KiB，并拒绝作为身份文件的重解析点。损坏、超限或账号不匹配的记录会报错，不按其中的 PID 盲目停止进程。

停止时先确认仍存活的进程与记录匹配，再请求正常停机；正常停机失败或等待超时后，才按既有规则终止持有句柄所指向的进程。访问被拒绝不等于进程已退出，不能因此覆盖身份记录或启动另一个后台。

## 类型与授权

`src/service` 定义请求类型、固定设置和客户端。服务参数不允许通过未知字段夹带新的路径或权限。账号运行身份来自选中的配置；入口不能用任意任务参数覆盖它。

MCP 的 HostSettings 由宿主启动参数建立，不属于工具参数。工具级调用不能注入输出根或媒体下载授权。Web 保留 Host、Origin、令牌和 CSRF 校验；底层 daemon 认证不替代 HTTP 防护。

## 查询与阻塞工作

daemon 按需准备查询状态，管理接口不依赖数据库初始化成功。MCP/其他业务编排不持有贯穿文件发布的数据库租约；需要查询时调用进程内分发，不向自身进行管道调用。

阻塞操作在受监督的执行上下文中运行。内部 worker 使用类型化请求，不重新解析公开 CLI，也不递归提交同一个操作。

每个 Web 服务实例的查询适配层共享 4 个执行名额，另允许最多 8 个请求等待。执行名额用尽后，获取名额最多等待 2 秒；等待队列已满、等待超时或名额通道关闭时返回 busy，对应 HTTP 429，而不是把繁忙误报为数据库不可用。等待取消会释放等待名额，已经发出的请求不会因超时或断连自动重放。2 秒只限制排队，不是整个查询的执行期限。这是进程内的有界等待，不是持久任务队列；图片解码还受后台自身的资源限制。

daemon 内部的 Web 查询同样在取得现有 4 个查询许可时最多等待 2 秒，保留类型化 busy，不发送或重放已取消、超时的排队请求。调用全程受现有 32 个 calls 许可约束，取消后仍继续的图片任务另受最多 2 个 decodes 许可约束，监控为单实例；这些是内部工作项的容量边界，不是把外层 8 个等待名额扩大为 32 个。

## 图片失败与重试

daemon IPC `DecodeImage` 的静态业务码 3 在 Web 层映射为 HTTP 503、`code: decode_failed`。内联图片遇到 `decode_failed` 时显示失败占位并保留手动重试，不自动重试；其他 503/429 仍按现有有限重试策略处理。不能仅凭 HTTP 503 判断为可重试繁忙，也不能把历史记录或标签查询的 503 当作预期图片解码失败。

V2 图片需要可用的图像 AES 密钥。缺少该必要条件时，真实 V2 解码成功验收应记录为人工阻塞并跳过，不计为成功；不会为完成验收自动取钥。已核验图片的预期失败响应只证明对应错误处理，不证明解码成功。

## 监控与增量状态

`wx monitor` 由 `src/cli/monitor_native.rs` 转交 `operation_client::run(Operation::Monitor)`，保留前台操作的生命周期。`src/application/monitor/transport.rs` 的查询传输层只连接现有 daemon，不负责启动；不能据此推断整个 CLI 命令不会启动后台。

普通账号管道请求上限为 64 KiB。首次基线与能放入单帧的请求直接走该管道；序列化后连同换行超过上限的完整 `NewMessages` 状态通过认证服务的 `Begin`、`Chunk`、`Finish`、`Abort` 传输。认证请求单帧仍不超过 64 KiB，每块条目 JSON 不超过 48 KiB、最多 1024 项。只有全部块的顺序、数量及字节数校验通过，才用完整状态调用一次查询；不丢弃会话、不拆分查询，也不改变新会话回退和全局 limit 的含义。

单份状态最多 8 MiB、100000 个会话，会话标识最多 512 个 UTF-8 字节。每账号最多容纳 2 份活动上传，执行中的查询仍占配额。状态仅在内存暂存，创建后固定 60 秒到期，续传不延长期限；服务每秒检查过期状态。失败、取消或超时后，客户端对已知上传 ID 尽力发送 `Abort`，清理最多额外等待 1 秒；若 `Begin` 响应丢失而未获得 ID，则由到期清理回收。daemon 停机清理暂存状态。

计时字段 `serialize_ms`、`connect_ms`、`write_ms`、`wait_read_ms`、`parse_ms` 可为 `null`。单帧路径记录这五项；认证分块记录 `serialize_ms` 和 `authenticated_roundtrip_ms`，未单独测量的连接、写入、等待读取及解析项为 `null`。统计忽略未测量项，不填 0；`wait_read_ms` 包含后台排队、缓存准备、查询和传输，不是纯查询耗时。

以上是 monitor 的 `NewMessages` 状态传输，不是 MCP `get_new_messages` 工具的会话摘要游标。

## 取消与停机

前台操作失联后按租约取消。持久任务退出等待客户端后仍可继续，必须显式取消任务。MCP 在途调用受截止时间约束，stdio 同步处理不保证能同时读到后续取消通知。

MCP 可选后台任务工具调用现有 `Configure/Submit/List/Get/Cancel/Events`，不把 stdio 搬入 daemon，也不经过查询会话的回收逻辑。MCP 断连不取消已接受任务；完整授权和幂等约束见[任务契约](daemon-tasks.md#mcp-任务工具)。

普通 worker 在创建挂起进程时通过 Job 列表属性原子分配到 Job，随后恢复执行，结束时回收后代。仅显式 force/account/restart 捕获允许用户应用脱离 Job；worker 仍被回收。不要把该例外用于无关子进程。

daemon 停机先停止接收、取消和排空执行，再释放句柄和会话。取消不回滚已发布文件；超时后重试写入操作前应检查产物。

## 源码导航

- `src/service/operation_client.rs`、`operations.rs`：前台调用与类型。
- `src/daemon/operation_service.rs`、`operation_worker.rs`：租约、输出和 worker。
- `src/daemon/tasks`：持久任务。
- `src/daemon/mcp_service.rs`、`mcp_rpc.rs`：MCP 编排和查询接线。
- `src/daemon/web_service.rs`：Web 业务。
- `src/daemon/operations`：执行宿主与操作装配；业务契约、用例、微信适配和基础设施分别由 `src/business`、`src/application`、`src/adapters/wechat` 和 `src/infrastructure` 提供。

参见[架构](architecture.md)、[任务契约](daemon-tasks.md)、[MCP 协议](../src/mcp/PROTOCOL.md)和[测试说明](../tests/README.md)。

## Web 查询传输

联系人、会话和标签成员 HTTP 查询使用共享 `service/query_client` 的 `connect_query`、`write_query` 和 `decode_query_response`，执行对端进程验证、协议与运行身份匹配及有界帧读取。该路径只连接已存在的 daemon，不回退裸报文、不自动启动或重发请求。

Web 固定 `RuntimeContext`，执行 Origin、令牌与 CSRF 检查。查询槽等待最多 2 秒，完整请求期限为 20 秒（Ping 为 1 秒），响应上限为 8 MiB；历史查询使用 Web RPC。业务错误与传输错误分别投影到 HTTP，不把后台不可用显示为空结果。
