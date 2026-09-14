# MCP 协议与宿主契约

## 边界与 SDK

`src/mcp/protocol.rs` 负责协议；`src/cli/mcp.rs` 通过 `wx mcp` 提供 stdio、参数封送与认证 RPC 适配。首次业务调用根据显式 `WX_CLI_CONFIG` 固定路由。原有查询与同步媒体工具发送 `service::protocol::Call::Mcp`；其账号读锁、宿主授权、路径守卫、缓存、ASR 与媒体发布均由 `daemon/mcp_service` 持有和执行。可选任务工具经 `cli/mcp_tasks` 调用现有任务 RPC，不经过查询会话，不在 MCP 内执行任务。初始化和工具列表不读取账号。编辑或切换账号前须退出 MCP 会话。
默认 17 个工具均已注册并有源码执行链，不等于旧 17 个工具全部语义迁移。默认列表中 14 项只读，`decode_image`、`decode_voice` 与 `transcribe_voice` 标记非只读；只有 `transcribe_voice` 的 openWorldHint 为 true，宿主可显式授权云端上传，配置式 Python 命名模型也可能按需下载权重。初始化与列表不读取凭证、模型或账号，不创建目录。任务工具的增量发现与授权见下文。

### daemon 授权与会话

认证服务检查运行身份、私有令牌和本机服务进程身份。宿主启动配置经内部 `HostSettings` 传送，不成为 MCP 工具参数；`Call`、`HostSettings`、语音参数及预算均拒绝未知字段，daemon 只允许 MCP 业务请求，不允许借此调用管理操作。路径转绝对值属于入口封送，文件读取、授权判定与提交守卫仍在 daemon。令牌持有者属于可信本机宿主，此边界不是针对同用户恶意进程或管理员的沙箱。

原有查询/同步媒体会话首次建立时绑定账号、配置读锁、宿主设置及拥有者进程句柄；后续调用不得重新授权或更换账号。EOF 尽力发送关闭，拥有者进程退出后 daemon 定期回收读锁。daemon 重启后旧查询会话拒绝继续，须重新启动 MCP；不把旧会话自动重建为新的授权会话。daemon 停机先取消并排空在途执行，再释放会话锁；已运行的有界 ASR 仍须退出，不能直接 abort 排空任务。持久任务不采用这套会话租期，重启后只恢复历史，不自动续跑。

内部请求携带原始请求 ID、响应预算和绝对截止时间，daemon 只收紧剩余期限。查询走 daemon 内部回调，不向自身发起 MCP/query IPC。stdio 仍同步处理，尚不支持读取后续通知来取消当前阻塞调用；超时或连接断开不回滚已发布文件，也不保证恰好一次执行。

协议实现使用同步 serde 核心，依赖已有 serde、serde_json 和 chrono。同步传输的取消限制见下文。

## 宿主启动参数

| 参数 | 当前默认值与约束 |
| --- | --- |
| `WX_CLI_CONFIG` | 必须由宿主显式设置；首次查询固定账号和配置读锁，不由工具参数选账号 |
| `--max-frame-bytes` | 默认 1048576，允许 1024..16777216；MCP 双向帧上限不含结尾 LF，其他普通 IPC 响应也受该预算约束 |
| `--media-output-root` | 无默认，必须已存在且可信；图片/语音解码发布必需，不自动创建 |
| `--image-key-file` | 旧明文密钥文件接口不再支持；显式设置时拒绝图片请求，不读取文件 |
| `--backend` | 默认 `local` 指 whisper.cpp，要求 `--whisper-binary` 和 `--whisper-model`；云端选 `explicit-open-ai` |
| `--language`、`--threads`、`--timeout-seconds` | 默认 `auto`、原生自动且最多 8、120 秒；显式线程和超时须正值。Python 未给线程时沿用 PyTorch 默认。MCP context 默认 30 秒，实际剩余期限会收紧后端预算，120 秒不延长此请求期限 |
| `--temp-root` | 普通 whisper.cpp 可指定可信目录，省略时 host 创建请求独占 TempDir；配置式 Python 不接受此用户参数，使用 host 私有目录 |
| `--allow-upload`、`--openai-base-url`、`--openai-model`、`--api-key-file` | 云端必须全部显式给出；不读默认 key/环境凭据，不自动回退。本地与云参数不可混用 |
| `--configured-local-python` | 默认 false；必须由宿主显式启用，且固定配置必须明确为 `transcription_backend="local"`。`local_whisper_model` 缺失默认 `base`；拒绝 cpp 路径、云参数和用户 temp-root，仅允许 local 后端 |
| `--voice-cache-file` | 可选显式 JSON 缓存路径，省略不持久化；父目录可信、存在且通过输出守卫，账号取固定 RuntimeContext.id |
| `--tasks`、`--task-kind` | 默认关闭；前者授权管理当前账号任务，后者为可重复/逗号分隔的提交类型白名单 |
| `--task-allow-media-write`、`--task-allow-memory-scan` | 默认 false；分别授权固定任务目录媒体写入、取钥任务扫描；扫描还须逐次确认 |
| `--task-allow-transcription`、`--task-allow-upload`、`--task-allow-media-download` | 默认 false；后台转写、云上传和 SNS 下载分别授权，不借用同步语音授权 |
| `--task-image-cache-dir` | 可选宿主路径，经共享 Configure 绑定；模型不能设置或覆盖 |

CLI 帮助还继承全局 `--with-meta` 和 `--help`；它们不是 MCP 工具 schema 属性，工具请求不能借此增加未公布参数。

**与兼容批处理区别：** `wx toolkit transcribe-chat` 等入口在用户请求转录后按固定配置选择引擎，配置缺少 `transcription_backend` 时默认 `local`（Python Whisper），缺模型时默认 `base`。这是兼容默认选择，不是失败回退；MCP 不沿用这个缺省后端规则，必须满足上表开关与显式配置双重条件。详情见 [本地 ASR](../toolkit/asr/LOCAL.md) 与 [云端授权](../toolkit/asr/OPENAI.md)。

## 后台任务工具

`--tasks` 增加 `list_tasks`、`get_task`、`cancel_task`、`get_task_events`；只有共享 daemon 能力与宿主授权的交集非空时才增加 `submit_task`。提交工具的 `kind` 枚举和选项仅展示允许的能力，执行仍使用共享类型、参数校验、配置绑定与幂等记录；模型不能通过布尔值获得宿主未给的授权。新增工具不会改变原查询路由和同步语音执行方式。

`submit_task` 接收 `{idempotency_key, kind, options?}`，立即返回 daemon 接受的任务记录，不等待执行完成。`get_task`、`cancel_task` 接收 `{id}`，列表为 `{}`；事件为 `{after?, limit?, wait_ms?}`。任务结果保留在 `structuredContent` 及 JSON 文本 content；业务失败为 `isError` 与 `structuredContent.error.code`，形状错误为 `-32602`。取消/超时的工具上下文不等于后台取消；响应丢失后先按原键查询或用原键、原参数重试。

账号和配置指纹与查询入口共用固定 `RuntimeContext`；后台 Configure 返回的绑定指纹也必须匹配。MCP 不维护任务数据库、队列、worker 或历史。完整启动示例、工具请求、授权矩阵、事件游标和 `interrupted` 语义见 [daemon 任务契约](../../docs/daemon-tasks.md#mcp-任务工具)。

## API

- 保留 `Protocol::new/handle/serve/phase`，原 `FnMut(Request) -> Result<Response, DispatchError>` 继续工作。
- 新增 `Dispatcher` trait 和 `Controlled(F)`：后者包装 `FnMut(Request, &CallContext) -> Result<Response, DispatchError>`。
- `handle_with_context(bytes, &CallContext)` 允许宿主注入当前请求的取消/截止时间。
- `CallContext::new(CancellationToken, Duration)`、`check()`、`remaining()`、`cancellation()`、`check_text_result(text)`；默认30秒。最后一项使用当前真实请求 ID、JSON 转义、content/isError 包装和 serve 的实际响应预算，不是固定开销估算。Duration 溢出按立即超时处理。
- `CancellationToken` 可克隆，可从外部线程 `cancel()`；无全局账户/取消状态。
- `tools()` 给出 17 项白名单/schema；`route(name,args)` 生成单条 Request，不包含后处理；宿主参数经 CLI 封送，图片路径检查和语音 prepare/bind/finish 由 `daemon/mcp_service` 执行。
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

| 已注册旧工具 | 参数与真实Request | 返回差异 |
| --- | --- | --- |
| get_recent_sessions | limit默认20 -> Sessions，1..500 | JSON编码的MCP text，保留此前接口；非旧Python中文排版 |
| get_contacts | query省略/空串、limit默认50 -> Contacts(legacy_view=true) | 已接固定账号 contact.db；返回 contacts/total，每行含 username/nick_name/remark/display/alias/description/phone。按 username/nick_name/remark 小写子串搜索；JSON-text，不是旧中文排版 |
| get_chat_history | chat_name -> chat，limit默认50、offset默认0；日期与类型转换如下 -> History | 保留JSON-text；默认选最新页，oldest_first=true 选最早页；两者最终按时间升序展示，不保证旧内容格式器逐字一致 |
| search_messages | keyword必填、chat_name为null/字符串/字符串数组 -> chats；默认limit20/offset0；日期转换；取offset+limit候选再全局裁剪results | JSON-text/count更新，非旧中文标题及分页提示；模糊匹配和同时间排序依赖原生查询 |
| decode_transfer | chat_name/local_id/create_time默认0 -> DecodeTransfer | JSON-text；exit_code非零改为安全工具错误，不回显text内部数据；不执行付款 |
| decode_location | 同上 -> DecodeLocation | 同上，成功保留原生结构与文本 |
| get_new_messages | 无参数 -> Sessions(limit=10001)，**不用语义不同的NewMessages** | 中文text：首轮未读会话摘要，之后发生时间变化的会话摘要；每个Protocol独立游标，不是全量消息流 |
| get_chat_images | chat_name/limit默认20/offset默认0/start_time/end_time或since/until -> Attachments(kinds=[image],image_metadata=true) | 已接资源元数据查询与白名单投影：local_id/create_time/md5/size/resource_status/size_status/size_kind/binding；缺失与歧义用状态及 null 表达，不补造数值；不是明文图片摘要/大小，也不解码或回传任意附件路径 |

参数规则：

- 原六工具的since/until Unix秒、msg_type数字及搜索chats扩展保留。
- start_time/end_time按旧Python的服务器本机时区，支持YYYY-MM-DD、YYYY-MM-DD HH:MM、YYYY-MM-DD HH:MM:SS；仅日期end取23:59:59，空串无限定。
- DST歧义/空洞报安全参数错误，要求Unix秒。非空旧日期与对应新边界同时指定拒绝。
- 搜索旧chat_name去两端空白/空项/重复项，空值表示全库；同时传chats拒绝。不接受旧Python宽松的非字符串列表项str()转换。
- msg_types null/[]无限定，支持text=1/image=3/voice=34/namecard=42/video=43/emoji=47/location=48/app或file=49/voip=50/system=10000；忽略大小写/两端空格，去重同义类型。
- 一个不同 msg_types 映射 msg_type，多个映射原生 msg_types 数字列表；非空列表与显式 msg_type 冲突拒绝。oldest_first 默认 false，true 从全分片最早消息分页。先在每片过滤并取 offset+limit 候选，再合并选页；不是反转原最新页。
- 新多类型/最早页路径中的数字 base 类型匹配低 32 位，完整高位类型精确匹配；前者保留 Rust 的扩展行为，旧 Python 多类型 SQL 使用完整值精确匹配。默认单类型路径仍保留原 Rust 过滤规则。同时间行沿用稳定排序，不声称 SQLite 的同时间行次序跨快照恒定。
- 字符串最大4096字符、数组100项、limit 1..500，offset通常0..1000000，搜索offset+limit不超过10000。旧History允许更大limit，本实现故意保留安全上限。
- 未知属性拒绝；null仅在公布的旧参数分支允许；history/decode/images的chat_name不得空白。

`get_chat_history` 不要求每个合法联系人都有普通消息表。只有全部已知消息分片成功扫描且未发现未知消息库时，才允许对没有对应表的会话返回空结果；没有可扫描的消息库、缺失分片或 SQL 错误仍返回错误。首次读取较大冷库可能超过调用期限；暖缓存成功不能证明冷启动在同一期限内可用，超时也不代表缓存文件与索引已同步完成。

工具整数边界不能混为一谈：transfer/location/refer 的 local_id/create_time schema 允许 i64，create_time 默认 0；file/record 的 local_id 必须为正，create_time 允许 i64 且默认 0，record 的 item_index 为 0..i64::MAX；image 的 local_id 为正且 create_time 为 0..i64::MAX；两个语音执行工具仅接受正媒体 local_id，不接受 create_time。业务层仍会校验实际消息身份。

联系人兼容读取保留数据库扫描顺序、不额外排序；存在精确 `local_type` 列时按 SQL `local_type != 3` 排除 3 和 NULL，缺列则保留全部，不再仅限私聊。display 优先 remark、nick_name、username；可选 alias/description/phone 列缺失用空串，搜索不扩展到这些可选字段。最多扫描 100000 行，单字段与 query 另有 4096 字节限制、已选结果 16MiB 上限，最终仍受 MCP 帧预算约束。无任何合格源行报错；有源行但搜索不匹配可返回空列表。

轮询语义：所有会话查询10001作为截断探针，超过10000失败；逐行验证必需字段和唯一username。首轮记录全部时间戳，仅显示unread>0；之后timestamp大于旧值才展示，新会话基准0，按时间升序。
空快照下一轮仍按首次处理，与旧空字典逻辑一致。删除的会话从快照移除。原生Sessions的群昵称/分类/类型标签可能与旧Python不同，不保证逐字一致。
后端错误、取消、超时、畸形/超大响应不推进游标；serve序列化/write/flush失败回滚该次游标并退出。
handle返回后已提交，外部transport发送失败应丢弃Protocol，不提供网络ACK或恰好一次保证。更换账户必须新建Protocol。

## 新增只读工具

以下六项已注册并接入只读 IPC。联系人标签返回名称、成员数和总关联数，不输出全部标签成员；成员通过独立工具按标签查询。引用和附件查询共享严格消息定位，要求完整分片清单及唯一消息，拒绝未知分片、同名视图、虚拟表和无 rowid 的消息表。

| 工具 | 参数与行为 |
| --- | --- |
| get_contact_tags | 无参数 -> ContactTags，返回 tags/name/member_count、total_tags、total_associations |
| get_tag_members | tag_name -> TagMembers，精确优先再模糊匹配，歧义拒绝 |
| decode_refer | chat_name/local_id/create_time默认0 -> DecodeRefer，结构化回复正文及引用对象 |
| get_voice_messages | chat_name/limit默认20/offset默认0/start_time/end_time或since/until -> VoiceMessages，返回 voices/count；真实音频大小未知时保持 null |
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

## 宿主语音执行

两项工具已实际注册。工具参数不能携带输出路径、后端、模型、凭证、缓存路径或上传许可，未知属性拒绝。

| 工具/参数 | 当前接口与语义 |
| --- | --- |
| decode_voice(chat_name,local_id) | `DecodeVoice {chat,local_id}`；local_id 为正数媒体 ID，不是 message_local_id。host 要求预存 `--media-output-root`，原生 SILK 解码为 24kHz 单声道 PCM16 WAV，守卫复核后不覆盖发布 |
| transcribe_voice(chat_name,local_id) | `TranscribeVoice {chat,local_id}`；host 默认显式 whisper.cpp；也支持上表的宿主配置式 Python；云端须 explicit-open-ai + allow-upload + 显式端点/模型/凭证，均不自动失败回退 |

执行链：`cli/mcp → authenticated Call::Mcp → daemon/mcp_service → voice::Args::prepare → 固定账号 → Pending::bind → 内部查询回调 → mcp_audio::q_prepare_voice → prepared_audio → Pending::finish`。准备音频、解码、识别、读取显式 ASR 凭证与 WAV 发布均在 daemon 中完成；下文 host 指 daemon 内的宿主策略执行器，不再指 stdio 进程。原始音频上限 16 MiB，内部准备结果上限 24 MiB，公开 MCP 帧预算不因此扩大。执行器验证版本、尺寸、SHA-256、SILK 和关联证据，并核对请求的 media_local_id；prepared_audio 不经过 stdio，也不对工具调用方公开。

host 路径先拒绝原始 `..` 再转绝对路径；共享 `local_files::HostOutputGuard` 隔离源库、解密/运行缓存、配置、密钥和后端输入。未显式设置本地 temp-root 时，在 prepare 创建独占子目录，由 Pending 持有 TempDir，先释放守卫再清理目录。初始化与列表不执行 prepare。默认不写永久 WAV；本地转录可使用受控临时 WAV。

`decode_voice` 的文件名为完整 WAV SHA-256 加 `.wav`。`audio::publish::publish_wav_noclobber` 在同目录暂存、sync、重读验证后，在提交前回调中检查真实 `check_text_result`、剩余期限及绑定账号；随后再复核守卫和暂存文件，最后禁止覆盖发布。成功模板为 `解码成功!\n  文件: {path}\n  时长: {seconds:.1}秒\n  大小: {逗号分组字节数} bytes`。`transcribe_voice` 成功模板为 `[{本机时区 YYYY-MM-DD HH:MM}] ({language})\n{text}`。协议取 `mcp_text` 的字符串作为 MCP text，不将此模板再编码成 JSON 字符串；空转录文本允许保留。

后端构造及 IPC 之后重新检查 context，再取实际 remaining 收紧 LocalConfig.timeout、LocalPythonConfig::tighten_timeout 或 OpenAiTranscriber::tighten_timeout；只缩短，不重新授予完整超时。Python 的绝对截止时间覆盖初始化、缓存身份和识别。`--voice-cache-file` 显式启用缓存，使用绑定 RuntimeContext.id，不接收工具提供的账号标签；未配置时调用现有字节转录。授权先于缓存读取，成功空文本可以命中。消息来源、时间、音频摘要和识别配置仍参与缓存身份，**执行 timeout 不再参与本地成功缓存键**：旧配置摘要记录不删除，但不立即命中新摘要。

两个本地后端已共享 `asr/windows_supervision.rs`：`OwnedHandle` 持有 Job，使用现有 windows 类型绑定；共用有界 PeekNamedPipe 读取，每管道每轮最多 64KiB，累计上限仍由各后端控制。Job 启用 KILL_ON_JOB_CLOSE，终止后最多等待 2 秒归零；Python 成功请求保持 worker 复用，错误或释放时回收。Python 在 Job 握手后才导入第三方模块，命名模型可能下载；这是可信本地推理环境，不是对任意程序的权限沙箱。完整预算、Python 包/模型边界见 [LOCAL.md](../toolkit/asr/LOCAL.md)。

缓存提交边界：host 已调用 `cached::transcribe_cached_with_receipt_checked`，识别后以完整成功模板预检；checked 缓存发布在暂存、sync 和快照复核后、实际 persist 前再次回调。host 核验真实请求 ID 的 `check_text_result`、原始路径守卫、context 和账号 before_commit。预算超限、取消或账号/守卫拒绝时，不发布本次缓存，保留原 DispatchError；普通 CLI 缓存 I/O 失败仍为独立状态，不丢弃成功识别。receipt 命中是只读，不新增缓存条目。

持久命中：绑定账号后，daemon 内 host `try_cached → receipt::lookup_success` 可在读取语音来源前，按精确 username + media_id 查询可信成功 receipt。匹配账号、后端配置、历史证据及记录摘要后，复核守卫、context、账号与完整响应预算，直接返回历史成功文本；不读取已删除的源语音、不再次识别、不修改缓存。whisper.cpp 仍需读取程序/模型计算身份；Python 身份可能启动解释器并导入依赖，但不需要加载模型执行识别，不能笼统称为“不启动进程”。缓存命中也要求 daemon 可用；daemon 重启后，旧 MCP 会话拒绝，新 MCP 会话重新授权后可以命中保留的成功 receipt。显示名不读取历史别名，必要时仍经内部 ResolveChat，再按解析后的 username 查询。Miss/Conflict/Unavailable 不冒充成功，继续受控源查询。**无源弱缓存不能自动补造身份；源仍存在时，已有强身份成功缓存可在真实 identity/evidence 核验通过后补写 receipt 索引**，不必重新识别。receipt 不是签名，不证明当前源消息仍存在或同路径账号数据未被替换。

限制：真实模型质量、账号完整性、GPU、云服务和安装部署须单独验证。WAV 与缓存提交前检查只拦截该提交点可知的拒绝，不保证之后的取消、IPC/stdout 断开可回滚。图片发布后响应失败仍非事务；均不承诺恰好一次、自动重试成功或 ACK 回滚。

History 多类型与最早页已接入 IPC、MCP 和 CLI；CLI 保留 --type，并有 --types 与 --oldest-first。Contacts 昵称/备注与旧范围查询已接入；Search offset 仍由协议扩大候选并裁剪，原生 Request::Search 没有 offset/cursor。旧 Python 渲染失败后补取候选的语义没有完整逐字兼容保证，不将选行兼容等同内容格式逐字兼容。

## 安全与协议

本节的媒体/云端参数限制指原有同步工具。可选后台任务的 `allow_upload` 等布尔值只是逐任务确认，仍须对应的宿主 `--task-allow-*` 授权及共享配置校验，不能给同步工具新增权限。

错误仅固定公开类别Unavailable/Internal/Cancelled/TimedOut/QueryFailed/InvalidResponse/ResultLimit。
Response.ok=false、Response.error存在、data.exit_code非零或类型不合法、data.error非null统一返回Query failed，不复制message/text/keys/路径/SQL/错误链。
成功查询内容是用户请求的数据，不做通用机密扫描；宿主不得把凭据混入成功 payload。`get_chat_images` 白名单包含 md5/size，资源状态可为 found/missing/ambiguous/md5_missing/unavailable/message_ambiguous，大小状态为 available/missing/ambiguous/not_requested/unavailable。`size_kind=encrypted_dat_metadata`、`binding=exact_resource_standard_filename_metadata` 明确其资源/标准文件名元数据语义，不升级为明文哈希认证；不合法字段组合拒绝。`decode_image` 由 daemon 按宿主授权写盘。认证 `Call::Mcp` 携带宿主显式后端、凭证文件路径与上传许可，凭证内容由 daemon 有界读取；内部音频查询只提供音频与证据。仅 daemon 内 host 的显式云端后端可在授权后上传，不公布工具级后端、许可或密钥参数。

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

模型质量、GPU、真实云服务、私人账号完整性和安装部署须另行核验。架构与前置条件见[架构说明](../../docs/architecture.md)及[工作流与前置条件](../../docs/legacy-workflow-gap-audit.md)。
