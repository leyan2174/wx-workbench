# MCP 协议与宿主契约

## 边界与 SDK

`src/mcp/protocol.rs` 负责协议；`src/cli/mcp.rs` 通过 `wx mcp` 提供 stdio、参数封送与认证 RPC 适配。首次业务调用根据显式 `WX_CLI_CONFIG` 固定路由。原有查询与同步媒体工具发送 `service::protocol::Call::Mcp`；其账号读锁、宿主授权、路径守卫、缓存与媒体发布均由 `daemon/mcp_service` 持有和执行。可选任务工具经 `cli/mcp_tasks` 调用现有任务 RPC，不经过查询会话，不在 MCP 内执行任务。初始化和工具列表不读取账号。编辑或切换账号前须退出 MCP 会话。
默认注册 15 个工具，其中 14 项只读，`decode_image` 为宿主授权的图片写出工具。初始化与列表不读取账号或密钥，不创建目录。任务工具的增量发现与授权见下文。

### daemon 授权与会话

认证服务检查运行身份、私有令牌和本机服务进程身份。宿主启动配置经内部 `HostSettings` 传送，不成为 MCP 工具参数；`Call`、`HostSettings` 及预算均拒绝未知字段，daemon 只允许 MCP 业务请求，不允许借此调用管理操作。路径转绝对值属于入口封送，文件读取、授权判定与提交守卫仍在 daemon。令牌持有者属于可信本机宿主，此边界不是针对同用户恶意进程或管理员的沙箱。

查询/同步媒体会话首次建立时绑定账号身份、宿主设置及拥有者进程句柄；配置读锁仅在每次操作期间持有，操作结束释放；后续调用不得重新授权或更换账号。EOF 尽力发送关闭，拥有者进程退出后 daemon 定期回收读锁。daemon 重启后旧查询会话拒绝继续，须重新启动 MCP；不把旧会话自动重建为新的授权会话。daemon 停机先取消并排空在途执行，再释放会话锁。持久任务不采用这套会话租期，重启后只恢复历史，不自动续跑。

内部请求携带原始请求 ID、响应预算和绝对截止时间，daemon 只收紧剩余期限。查询走 daemon 内部回调，不向自身发起 MCP/query IPC。stdio 仍同步处理，尚不支持读取后续通知来取消当前阻塞调用；超时或连接断开不回滚已发布文件，也不保证恰好一次执行。

协议实现使用同步 serde 核心，依赖已有 serde、serde_json 和 chrono。同步传输的取消限制见下文。

## 宿主启动参数

| 参数 | 当前默认值与约束 |
| --- | --- |
| `WX_CLI_CONFIG` | 必须由宿主显式设置；首次查询固定账号，逐次操作取得配置读锁，不由工具参数选账号 |
| `--max-frame-bytes` | 默认 1048576，允许 1024..16777216；MCP 双向帧上限不含结尾 LF，其他普通 IPC 响应也受该预算约束 |
| `--media-output-root` | 无默认，必须已存在且可信；图片解码发布必需，不自动创建 |
| `--image-key-file` | 旧明文密钥文件接口不再支持；显式设置时拒绝图片请求，不读取文件 |
| `--tasks`、`--task-kind` | 默认关闭；前者授权管理当前账号任务，后者为可重复/逗号分隔的提交类型白名单 |
| `--task-allow-media-write`、`--task-allow-memory-scan` | 默认 false；分别授权固定任务目录媒体写入、取钥任务扫描；扫描还须逐次确认 |
| `--task-allow-media-download` | 默认 false；显式授权朋友圈媒体下载 |
| `--task-image-cache-dir` | 可选宿主路径，经共享 Configure 绑定；模型不能设置或覆盖 |

CLI 帮助还继承全局 `--with-meta` 和 `--help`；它们不是 MCP 工具 schema 属性，工具请求不能借此增加未公布参数。

## 后台任务工具

`--tasks` 增加 `list_tasks`、`get_task`、`cancel_task`、`get_task_events`；只有共享 daemon 能力与宿主授权的交集非空时才增加 `submit_task`。提交工具的 `kind` 枚举和选项仅展示允许的能力，执行仍使用共享类型、参数校验、配置绑定与幂等记录；模型不能通过布尔值获得宿主未给的授权。任务工具与查询路由、同步图片执行相互独立。

`submit_task` 接收 `{idempotency_key, kind, options?}`，立即返回 daemon 接受的任务记录，不等待执行完成。`get_task`、`cancel_task` 接收 `{id}`，列表为 `{}`；事件为 `{after?, limit?, wait_ms?}`。任务结果保留在 `structuredContent` 及 JSON 文本 content；业务失败为 `isError` 与 `structuredContent.error.code`，形状错误为 `-32602`。取消/超时的工具上下文不等于后台取消；响应丢失后先按原键查询或用原键、原参数重试。

账号和配置指纹与查询入口共用固定 `RuntimeContext`；后台 Configure 返回的绑定指纹也必须匹配。MCP 不维护任务数据库、队列、worker 或历史。完整启动示例、工具请求、授权矩阵、事件游标和 `interrupted` 语义见 [daemon 任务契约](../../docs/daemon-tasks.md#mcp-任务工具)。

## API

- `Protocol::new/handle/serve/phase` 支持 `FnMut(Request) -> Result<Response, DispatchError>`。
- `Dispatcher` trait 和 `Controlled(F)`：后者包装 `FnMut(Request, &CallContext) -> Result<Response, DispatchError>`。
- `handle_with_context(bytes, &CallContext)` 允许宿主注入当前请求的取消/截止时间。
- `CallContext::new(CancellationToken, Duration)`、`check()`、`remaining()`、`cancellation()`、`check_text_result(text)`；默认30秒。最后一项使用当前真实请求 ID、JSON 转义、content/isError 包装和 serve 的实际响应预算，不是固定开销估算。Duration 溢出按立即超时处理。
- `CancellationToken` 可克隆，可从外部线程 `cancel()`；无全局账户/取消状态。
- `tools()` 给出 15 项白名单/schema；`route(name,args)` 生成单条 Request，不包含后处理；宿主参数经 CLI 封送，图片路径检查与发布 由 `daemon/mcp_service` 执行。
- `Dispatcher::task_tools()` 默认空；任务适配器补充宿主允许的工具，`dispatch_task()` 返回任务 RPC 转换的 MCP 结果，不经过 `route` 或查询执行器。
- **route不是完整工具执行**：搜索offset扩大候选limit，需要Protocol裁剪；轮询状态和图片字段投影也由Protocol完成。

接线示意（query_adapter由宿主实现，并非本仓库现有公共API）：

```rust
let mut protocol = Protocol::new(Controlled(|request, context: &CallContext| {
    context.check()?;
    query_adapter(request, context.remaining(), context.cancellation())
}));
let token = CancellationToken::default();
let context = CallContext::new(token.clone(), Duration::from_secs(10));
let reply = protocol.handle_with_context(frame, &context);
```

CLI 将固定账号与调用预算封装为认证 `Call::Mcp`，在发送前检查剩余期限，并通过 `request_with_timeout` 限制服务请求。后台就绪检查与调用预算分别约束；等待后仍需检查期限，不能把等待时间换成新的完整预算。读取响应先执行字节上限，再解析 JSON。
同步serve在callback运行期间不能读取后续notifications/cancelled；外部并发transport须按在途id关联token。没有实现异步stdio，不强杀不合作callback，不打断阻塞reader。
调用前后check，迟到成功结果被丢弃；协作callback可在等待/分批查询时check。callback panic不捕获，stdout不得写日志；宿主须自行保证stderr不泄漏私有数据。

## 工具参数与返回

本节 `get_new_messages` 返回会话摘要，使用每个 `Protocol` 自己的游标。它不接收完整 `NewMessages` 状态，也不暴露 monitor 的认证分块上传接口；两者不能互换。monitor 的帧限额、完整状态查询及回收语义见[监控入口](../../docs/daemon-entrypoints.md#监控与增量状态)。

| 已注册工具 | 参数与真实Request | 返回差异 |
| --- | --- | --- |
| get_recent_sessions | limit默认20 -> Sessions，1..500 | JSON编码的MCP text，保留此前接口；非旧Python中文排版 |
| get_contacts | 仅 query、limit；query省略/空串、limit默认50 -> Contacts(ContactsRequest) | 与 CLI/HTTP 共用当前账号查询租约的正式联系人业务；仅列真人，返回 contacts/total，每行仅 username/display。按 username 或首选 display 不区分大小写子串搜索，按 display、username 稳定排序；total 为截取前数量，limit=0 返回空列表及 total。MCP 只包 JSON-text；legacy_view（包括 false/null）及其他未知字段在 MCP 和 Contacts IPC 均明确拒绝，不保留旧视图 |
| get_chat_history | chat_name -> chat，limit默认50、offset默认0，since/until 为 Unix 秒；类型转换如下 -> History | 保留JSON-text；默认选最新页，oldest_first=true 选最早页；两者最终按时间升序展示，不保证旧内容格式器逐字一致 |
| search_messages | keyword必填，chats 为可选字符串数组；默认limit20/offset0，since/until 为 Unix 秒；取offset+limit候选再全局裁剪results | JSON-text/count更新，非旧中文标题及分页提示；模糊匹配和同时间排序依赖原生查询 |
| decode_transfer | chat_name/local_id/create_time默认0 -> DecodeTransfer | JSON-text；exit_code非零改为安全工具错误，不回显text内部数据；不执行付款 |
| decode_location | 同上 -> DecodeLocation | 同上，成功保留原生结构与文本 |
| get_new_messages | 无参数 -> Sessions(limit=10001)，**不用语义不同的NewMessages** | 中文text：首轮未读会话摘要，之后发生时间变化的会话摘要；每个Protocol独立游标，不是全量消息流 |
| get_chat_images | chat_name/limit默认20/offset默认0/since/until -> Attachments(kinds=[image],image_metadata=true) | 已接资源元数据查询与白名单投影：local_id/create_time/md5/size/resource_status/size_status/size_kind/binding；缺失与歧义用状态及 null 表达，不补造数值；不是明文图片摘要/大小，也不解码或回传任意附件路径 |

参数规则：

- since/until 只接受 Unix 秒整数；`start_time`/`end_time` 已移除，传入会作为未知属性拒绝。
- `search_messages` 只接受 `chats` 字符串数组限定会话；旧 `chat_name` 参数已移除。省略 `chats` 表示全库。
- msg_types null/[]无限定，正式词汇为text=1/image=3/voice=34/video=43/sticker=47/location=48/link或file=49/call=50/system=10000；忽略大小写/两端空格，去重同值类型。`emoji`/`voip`/`app`/`namecard` 已移除并拒绝。
- 一个不同 msg_types 映射 msg_type，多个映射原生 msg_types 数字列表；非空列表与显式 msg_type 冲突拒绝。oldest_first 默认 false，true 从全分片最早消息分页。先在每片过滤并取 offset+limit 候选，再合并选页；不是反转原最新页。
- 新多类型/最早页路径中的数字 base 类型匹配低 32 位，完整高位类型精确匹配；前者保留 Rust 的扩展行为，旧 Python 多类型 SQL 使用完整值精确匹配。默认单类型路径仍保留原 Rust 过滤规则。同时间行沿用稳定排序，不声称 SQLite 的同时间行次序跨快照恒定。
- 字符串最大4096字符、数组100项、limit 1..500，offset通常0..1000000，搜索offset+limit不超过10000。旧History允许更大limit，本实现故意保留安全上限。
- 未知属性拒绝；msg_types 允许 null 表示不限类型；history/decode/images的chat_name不得空白。

`get_chat_history` 不要求每个合法联系人都有普通消息表。只有全部已知消息分片成功扫描且未发现未知消息库时，才允许对没有对应表的会话返回空结果；没有可扫描的消息库、缺失分片或 SQL 错误仍返回错误。首次读取较大冷库可能超过调用期限；暖缓存成功不能证明冷启动在同一期限内可用，超时也不代表缓存文件与索引已同步完成。

工具整数边界不能混为一谈：transfer/location/refer 的 local_id/create_time schema 允许 i64，create_time 默认 0；file/record 的 local_id 必须为正，create_time 允许 i64 且默认 0，record 的 item_index 为 0..i64::MAX；image 的 local_id 为正且 create_time 为 0..i64::MAX。业务层仍会校验实际消息身份。

联系人兼容读取保留数据库扫描顺序、不额外排序；存在精确 `local_type` 列时按 SQL `local_type != 3` 排除 3 和 NULL，缺列则保留全部，不再仅限私聊。display 优先 remark、nick_name、username；可选 alias/description/phone 列缺失用空串，搜索不扩展到这些可选字段。最多扫描 100000 行，单字段与 query 另有 4096 字节限制、已选结果 16MiB 上限，最终仍受 MCP 帧预算约束。无任何合格源行报错；有源行但搜索不匹配可返回空列表。

轮询语义：所有会话查询10001作为截断探针，超过10000失败；逐行验证必需字段和唯一username。首轮记录全部时间戳，仅显示unread>0；之后timestamp大于旧值才展示，新会话基准0，按时间升序。
空快照下一轮仍按首次处理，与旧空字典逻辑一致。删除的会话从快照移除。原生Sessions的群昵称/分类/类型标签可能与旧Python不同，不保证逐字一致。
后端错误、取消、超时、畸形/超大响应不推进游标；serve序列化/write/flush失败回滚该次游标并退出。
handle返回后已提交，外部transport发送失败应丢弃Protocol，不提供网络ACK或恰好一次保证。更换账户必须新建Protocol。

## 只读内容工具

以下六项已注册并接入只读 IPC。联系人标签返回名称、成员数和总关联数，不输出全部标签成员；成员通过独立工具按标签查询。引用和附件查询共享严格消息定位，要求完整分片清单及唯一消息，拒绝未知分片、同名视图、虚拟表和无 rowid 的消息表。

| 工具 | 参数与行为 |
| --- | --- |
| get_contact_tags | 无参数 -> ContactTags，返回 tags/name/member_count、total_tags、total_associations |
| get_tag_members | tag_name -> TagMembers，精确优先再模糊匹配，歧义拒绝 |
| decode_refer | chat_name/local_id/create_time默认0 -> DecodeRefer，结构化回复正文及引用对象 |
| get_voice_messages | chat_name/limit默认20/offset默认0/since/until -> VoiceMessages，返回 voices/count；真实音频大小未知时保持 null |
| decode_file_message | chat_name/local_id/create_time默认0 -> DecodeFileMessage，仅查当前固定账号 msg/file 内原始副本 |
| decode_record_item | chat_name/local_id/item_index/create_time默认0 -> DecodeRecordItem，完整 datalist 的从零开始索引，仅查该联系人 Rec 内原始副本 |

附件返回 JSON-text，包含 `exit_code`、`status`、`metadata` 和 `reference`；状态区分 `found`、`missing`、`text`、`metadata_only`，非文件状态不扫描磁盘。外层文件 XML 上限 20,000 字节，转发记录 500,000 字节，不受 History 展示项数截断影响。根来自当前账号的显式数据库配置，不允许工具参数指定根目录；不下载、写入或额外解码图片、语音、视频。

引用有消息 MD5 时读取候选计算并匹配；没有 MD5 时，仅接受唯一候选且返回 `binding=heuristic` 及警告，实际计算出的 MD5 不会将弱匹配升级为已认证身份。同 MD5 多副本返回数量；无 MD5 多候选拒绝，不按时间任取。只读句柄维持到查询结果构造为 JSON 值，之后的 IPC 传输不持有该锁；返回路径不是永久锁或不可变副本。扫描条目、目录、候选和累计哈希读取均有限额；错误不是“缺失”。

## 图片解码：宿主受控写出

`decode_image` 的调用关系为 `route → cli/mcp → authenticated Call::Mcp → daemon/mcp_service::HostSettings::prepare_request → daemon 内部查询回调 → mcp_image::q_decode_image_with_key_file → native_image`。合成进程测试验证执行链，不代替真实账号或图片内容验收。

| 边界 | 当前契约 |
| --- | --- |
| 工具参数 | `chat_name` 非空白；`local_id` 为 1..i64::MAX；`create_time` 为 0..i64::MAX，默认 0 表示不限定时间，仍要求唯一消息 |
| 路由 golden | `{chat_name:"peer",local_id:7}` → `{cmd:"decode_image",chat:"peer",local_id:7,create_time:0}`；未注入时空 `output_root` 和 None `image_key_file` 不序列化 |
| 宿主输出 | `wx mcp --media-output-root <已存在的可信目录>`；不自动创建，未配置时在账号访问前拒绝；宿主相对路径转绝对路径，拒绝 `..`，daemon 再检查目录与受保护输入隔离 |
| 宿主密钥 | 不支持明文 `--image-key-file`，显式设置时拒绝请求；工具参数中的路径仍被剥离。Web 专用宿主入口从账号加密 Store 读取并在内存中传递材料，不创建明文临时文件 |
| 解码范围 | 当前账号的唯一图片消息、唯一资源库及受限 DAT 候选；完整分片检查、歧义拒绝；V2 无有效显式 AES 密钥失败，不调用自动 provider |
| 发布 | 同目录临时文件、`sync_all`、发布前复核、`persist_noclobber`；输出名为 `<decoded_md5>.<format>`，已有文件、链接或目录均不覆盖 |
| 外部行为 | 不下载、不上传、不执行外部转换器；`readOnlyHint=false`、`destructiveHint=false`、`openWorldHint=false`；不公布覆盖、上传或自动重试选项 |

密钥文件可省略以处理不需要外部 AES 密钥的格式；这不意味着 V2 会自动获取密钥。输出不能与受保护的账号输入、缓存、配置或密钥路径重叠。宿主应提供独立本地目录；这些路径防护不是对特权进程的通用权限沙箱。

成功返回仍是 JSON 编码的 MCP text，内部为 `exit_code:0`、`status:"published"`、`image`。图片对象含 `message` 身份、`path`、`source_path`、`resource_rowid`、资源/DAT/明文摘要、`size`、`format`、`decoder`、`candidates` 和 `binding`。`binding=resource_scan_filename_heuristic` 是关联证据，不是消息真实性或资源摘要等于明文摘要的认证。成功内容含本地路径，不含图片字节或密钥；不要把图片列表的字段投影规则误套到解码返回。

### 发布与响应不是同一事务

| 结果阶段 | 调用方应如何解释 |
| --- | --- |
| 参数或宿主预检拒绝 | 未进入图片导出；未知参数为协议参数错误，宿主配置错误使用固定安全工具错误 |
| 消息缺失、歧义、解码或发布失败 | 图片业务失败映射为 `Query failed`；server 捕获 Err 时使用传输成功外壳和 `{exit_code:3,status:"error",message:"Image export failed"}`，与明确的消息缺失码 1、身份歧义码 2 区分；协议仍按非零 exit_code 判定工具错误，不误报后端不可用；不暴露底层错误链，不覆盖已有目标。未分类的资源缺失也可能进入码 3，不能据此断言本地媒体存在 |
| 发布完成且响应送达 | 返回 `status:"published"` 与本地文件证据 |
| 发布完成但 IPC/MCP 超时、响应超限或传输失败 | 调用可失败或 stdio 退出，文件仍保留；不能据此声称没有副作用，也不会回滚已发布文件 |

输出响应限长与 deadline 检查发生在图片发布之后也可能失败；同步 stdio 不能在途取消，超时不保证中止 daemon 的阻塞导出。协议游标的失败回滚只涉及轮询状态，不涉及图片文件。宿主应先检查输出目录再决定是否重试；不承诺自动重试、重复调用成功、幂等成功或恰好一次。相同明文摘要再次发布可能因文件已存在而失败。

## 原始语音交付

`get_voice_messages` 是只读目录工具，不执行音频转换或识别。需要原始语音文件时使用 `wx voices`；完整聊天导出保留语音引用，并通过关联 manifest 把原始 SILK 交给下游工具。目录媒体 ID 不等于消息 ID，不应以编号相同推断关联。

## 安全与协议

同步图片工具与后台任务分别取得宿主授权。任务参数不能扩大同步工具的权限。

错误仅固定公开类别Unavailable/Internal/Cancelled/TimedOut/QueryFailed/InvalidResponse/ResultLimit。
Response.ok=false、Response.error存在、data.exit_code非零或类型不合法、data.error非null统一返回Query failed，不复制message/text/keys/路径/SQL/错误链。
成功查询内容是用户请求的数据，不做通用机密扫描；宿主不得把凭据混入成功 payload。`get_chat_images` 白名单包含 md5/size，资源状态可为 found/missing/ambiguous/md5_missing/unavailable/message_ambiguous，大小状态为 available/missing/ambiguous/not_requested/unavailable。`size_kind=encrypted_dat_metadata`、`binding=exact_resource_standard_filename_metadata` 明确其资源/标准文件名元数据语义，不升级为明文哈希认证；不合法字段组合拒绝。`decode_image` 由 daemon 按宿主授权写盘。认证 `Call::Mcp` 携带宿主媒体设置，工具参数不能覆盖这些设置。

基准：[MCP 2025-06-18基础](https://modelcontextprotocol.io/specification/2025-06-18/basic)、[生命周期](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)、[stdio](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、[工具](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)。

- 固定版本2025-06-18；initialize校验版本/clientInfo/capabilities，不支持版本回报固定版本供客户端断开；仅tools、listChanged=false。
- initialized通知推进Ready；ping前后可用；未Ready的tools报-32002，重复initialize -32600；list无分页，cursor拒绝。
- id保留字符串/i64/u64，null/小数/布尔/对象拒绝；不保存无限历史id。语法/UTF8错误-32700，错误envelope/批量-32600，未知方法-32601，参数/工具错误-32602，内部错误-32603；其他工具错误content+isError=true。
- 合法通知不回复不dispatch；无效JSON/envelope不是通知；客户端response envelope不支持。
- NDJSON不是LSP Content-Length，接受CRLF；默认双向1MiB（输入不含LF、含CR），超限退出。输出完整限长序列化后才write/flush，不因超限写半帧。
- 边界EOF正常，半帧EOF为UnexpectedEof；坏帧/空行回解析错误再继续；I/O错误传播。
- serde_json默认递归限制、重复字段后值覆盖；有限schema不是通用JSON Schema验证器。
- handle及handle_with_context只接已分帧数据，外部transport预限长；回调分配Response内存和阻塞时限由宿主负责。

## 测试

协议、守卫、查询和真实子进程分别验证，单个夹具通过不代表整条业务已验收。统一命令和可选依赖见[测试说明](../../tests/README.md)。协议测试覆盖握手、schema、帧预算、日期、轮询游标和安全错误；进程测试覆盖账号隔离、EOF、取消、停机及媒体发布。

私人账号完整性和安装部署须另行核验。架构与前置条件见[架构说明](../../docs/architecture.md)及[工作流与前置条件](../../docs/workflow-requirements.md)。
