# 协议入口与公共契约验收证据

本表保留开发阶段逐项核对的生产链路、测试断言与限制，不单独代表全仓验收。下文“待父集中验证”是编写时的历史状态，现已由根工程集中测试及独立夹具执行结果更新，详见[集中验收](architecture-final-validation.md)。源码审查、测试代码存在、测试执行成功是三种不同证据；首次失败和对应复测均保留，不能据旧 check 推定新用例通过。

## 1. 公共契约归属与薄入口

源码：`src/service/operations.rs` 的 `Operation`、`src/service/operation_protocol.rs` 的 `Invocation/Page`、`src/service/protocol.rs` 的 `Call/Submission/Task` 分别表达前台调用和持久任务协议；`src/service/operation_client.rs::run_inner` 负责参数校验、固定 RuntimeContext、发送/轮询、输出及退出码。普通查询继续使用 `src/ipc.rs` 的查询协议及 `src/service/query_client.rs`，不由 CLI 再运行 SQL。

业务实例：`src/business/voice/catalog.rs` 拥有 `Query/Entry/Page/Source`，只依赖标准库；`src/adapters/wechat/media/voice_catalog.rs::Catalog::read` 拥有 schema、SQL、排序和物理证据，`src/daemon/query/mcp_voice.rs::q_voice_messages` 只负责账号库存及异步缓存边界。server 收到业务 Page 后才显式转旧 wire。旧查询算法只有这一份生产实现。

测试：`src/business/voice/catalog/tests.rs::invalid_queries_do_not_read_the_source`、`incomplete_sources_and_invalid_pages_fail_closed`；`tests/fixtures/mcp-voice/tests.rs::typed_previews_and_legacy_wire_golden_preserve_duplicate_ids` 经过真实业务 list 和 SQLite Catalog，再检查协议投影。运行结果：**待父集中验证**。

例外：service/IPC 中可序列化的 DTO、JSON 响应是传输契约，不等于业务模型；本项没有宣称所有旧协议字段均已删除。CLI 保留参数、展示及宿主授权策略也不是重复业务实现。其它业务域的全面归属结论由对应审查者提供。

## 2. Web 三路由的实际协议修复

源码：`src/toolkit/web/mod.rs::router` 注册 `/api/contacts`、`/api/sessions`、`/api/tag-members`；对应 handler 经 `src/toolkit/web/query.rs::request` 到 `raw`。旧的裸 Request/Response 读写已替换为现有 `service::query_client::{connect_query, write_query, decode_query_response}` 和 `transport::framing::line`。

保证：先核验 OS 对端及 QueryHello 的版本/runtime，再发送带 runtime 与响应预算的 QueryEnvelope，最后验证 QueryReply 的版本/runtime。保持 Shared 中固定 RuntimeContext、查询槽位/等待人数限制、2 秒等待、普通 raw 请求 20 秒超时、8 MiB 响应上限。单次发送、不重放；Web 使用 connect-only，不因显式停 daemon 自动重启。

真实集成测试：`tests/fixtures/daemon-tasks/web_query.rs::three_web_query_routes_use_real_daemon_v3_and_stay_account_bound`，由同目录 `runtime.rs` 注册。启动真实 wx daemon/Web，使用合成人工密钥与 SQLCipher 联系人/会话库，通过 HTTP 检查三路由结果、分页及旧展示别名；两个账号交替查询仍各自固定；错用另一 Web token 为 401；无效 limit/额外 source 参数为 400；显式停止一个 daemon 后三路由均为 503，pid/token 不重建，另一个账号仍可查询。不是仅 mock transport 的测试。运行结果：**待父集中验证**。

例外：History 仍走既有 `Call::Web` 服务链路，这次三路由修复不替换它。连接失败/超时不保证后台从未执行；这里只保证不自动重放请求。端到端测试不等于覆盖所有超时及恶意帧组合。

## 3. 账号固定与入口授权

源码：`src/service/client.rs::connect_named` 读取固定 runtime 的身份记录，核对 runtime，再用命名管道服务端 PID 和 `verify_process` 验证进程；`request_with_timeout` 从该 runtime 读取服务 token 并核对回复 runtime/version。`src/service/query_client.rs::connect_query` 在这层对端验证上增加 query v3 握手。不能把公开的 runtime_id 单独视作授权凭据。

HTTP：`src/toolkit/web/mod.rs::security` 核对 Host/Origin/Sec-Fetch-Site；受保护路由要求启动 token；写入要求同源 POST 与 CSRF token。此次 raw 修复未绕过这些 middleware。

MCP 任务：`src/cli/mcp_tasks.rs::Args::{permits, authorize}` 根据进程启动参数限制任务种类及内存扫描、媒体写入、上传等能力；工具参数中的本次确认不能自行获得宿主未授予的能力。`src/service/mcp.rs::HostSettings/Call` 将宿主设置、固定 runtime 与 session 分开于工具请求，`open_session` 表达禁止重启后默默重绑的协议约束。

测试：上述 Web 真链路测试；`tests/fixtures/daemon-tasks/mcp.rs::mcp_tasks_reject_model_authorization_paths_and_changed_account` 实际启动 MCP，拒绝 shell/任意 output_dir/坏 ID，拒绝模型自授权，拒绝前不启动 daemon；修改固定配置后返回 configuration_changed，恢复原文件也不恢复已失效会话，跨账号配置替换同样拒绝，另入口设置冲突返回 settings_conflict。运行结果：**待父集中验证**。

例外：HTTP token、OS 管道对端身份与业务操作授权是不同层，不能互相替代；本项不重复父代理负责的密钥存储/发布安全验收，也不声称合法同用户进程完全不可信时仍构成隔离沙箱。

## 4. 普通查询生命周期

源码：`src/daemon/server.rs::{handle_connection_windows, dispatch_state}` 验证请求帧和预算；Ping 不初始化数据库，其它请求持有 `QueryState::snapshot` 返回的 QueryLease 直到 dispatch 返回。`src/daemon/query_state.rs::{snapshot, invalidate, shutdown}` 用代际读租约与写锁排空查询，并等待取消后仍在提交的缓存工作，再释放旧代际；不是每次请求重建另一套数据库服务。

测试：`src/daemon/query_state.rs::invalidation_drains_active_leases_and_queues_new_queries_but_not_ping` 实际持有旧租约，证明失效等待、新查询排队而 Ping 可用；`dispatch_retains_its_generation_lease_until_query_returns` 锁住 names 阻止返回，证明 dispatch 完成前 invalidate 不越过租约。运行结果：**待父集中验证**。

例外：查询不是持久任务，没有任务 ID、提交日志或重启重放。客户端 async 超时可关闭其 I/O，但 server dispatch 没有把客户端 EOF 当作立即取消信号；不能承诺断连即停止 SQL/已开始的缓存工作。server 有自身连接期限，缓存提交另由代际排空保证收尾。CLI/MCP 的 send_with_limits 可在发送前启动 daemon，Web raw 的 connect-only 是有意保留的不同策略。

## 5. 前台操作生命周期

源码：`src/service/operation_client.rs::run_inner` 生成调用 ID，发送 OperationStart，按序轮询 stdout/stderr 和退出码，结束或 Ctrl-C 后请求 OperationCancel。`src/daemon/operation_service.rs::Service::{start, dispatch, shutdown}` 管理内存中的调用、签名、有限输出、工作者及租约；`execute` 检查关闭、取消和租约过期。同 ID 同签名不重复启动，不同签名冲突，已释放且仍被记录的 ID 返回 consumed。

测试：`src/daemon/operation_service.rs::duplicate_requests_are_not_replayed_and_conflicts_are_rejected` 注入终态条目，验证重复 start 不创建 worker、不同环境签名冲突、释放后 consumed，以及停机准入拒绝。它是服务级合成测试，不冒称真实外部程序集成。运行结果：**待父集中验证**。

例外：前台操作依赖客户端轮询租约；断连不是持久化承诺，也不保证立即取消，需等待租约检查。输出/已消费记录有界，不提供跨 daemon 重启的任务恢复或无限期幂等；OperationStart 回复丢失仍可能有执行，不能因未收到回复而自行重新生成 ID 重试。

## 6. 持久任务、幂等与重启

源码：`src/daemon/tasks/mod.rs::Service::submit` 在固定配置 fingerprint 与 settings 下校验请求，用任务/绑定设置/fingerprint 计算签名；同 ID 同签名返回已有任务，不同签名冲突。先预约有界队列、持久化记录，再交工作队列；持久化失败撤回登记，不执行。`src/daemon/tasks/store.rs` 将恢复时的未完成记录置为 interrupted，不自动重新排队。`src/service/client.rs::request_with_timeout` 明确超时结果未知，保留原幂等键。

真实测试：`tests/fixtures/daemon-tasks/runtime.rs::daemon_task_worker_exports_once_and_history_survives_restart` 合成消息库导出，重复原提交只有一条任务，不同请求复用 ID 失败、另一账号不能 get，停止/重启后原结果仍可读；`web_and_cli_share_tasks_and_web_shutdown_does_not_stop_daemon` HTTP 重复 POST 返回同 ID，Web shutdown 后 daemon PID 不变且任务完成，重开 Web 可读原任务，显式停止 daemon 后 Web 不拉起它。

服务测试：`src/daemon/tasks/tests.rs::restart_marks_unfinished_tasks_interrupted_and_never_replays_them` 恢复 running 记录后原提交返回 interrupted，并断言工作队列为空。运行结果：上述测试均 **待父集中验证**。

例外：持久任务与前台操作不同，Web/MCP 客户端离开不等于取消后台任务。恢复为 interrupted 不是自动续跑；取消/崩溃不承诺撤销已经产生的外部副作用。历史有容量和终态淘汰，幂等不是无限期全局 exactly-once；重试必须使用同账号、原 ID、相同请求及兼容的固定配置。

## 7. CLI / MCP 类型过滤与旧 wire

源码：`src/cli/{history,search}.rs` 都调用 `src/service/message_filter.rs::cli_type`，`src/mcp/protocol.rs` 用同模块的 `mcp_type`。业务 `src/business/messages/filter_label.rs::FilterLabel` 拥有标签语义；`legacy_wire_type` 专属兼容投影拥有旧数字；`src/adapters/wechat/messages/read/mod.rs::LegacyReadPolicy/read_selection` 才解释存储筛选。daemon `src/daemon/query/message_read.rs` 没有把旧数字伪装成业务 Kind。

保证：CLI 保留大小写/空白敏感的允许值，不接受 app/namecard；MCP 保留 trim/大小写折叠、emoji/voip/app/namecard，仍拒绝 sticker/call/link。业务 file 与 app 语义不同，旧 wire 都可投影为 49；不能据同一数字推定业务分类相同。Sticker/Location/ContactCard 是窄 FilterLabel，不硬塞 Structured。

例外及精确差异：旧 Video=43，不因业务 Video 还覆盖 62 而扩大旧过滤；旧 System=10000，不自动包含业务 System 的 10002。适配器继续区分基础数字的低 32 位筛选和 packed/负数完整数字精确匹配。这是本次迁移保留的现行语义，不能宣称历史最早 AST 的 IN(49) 本来就包含所有高位。

测试：`src/service/message_filter.rs::tests::{cli_allowed_values_and_numbers_are_unchanged, mcp_aliases_case_and_intent_differences_are_preserved}` 逐值检查允许集、大小写、别名及数字差异；`tests/fixtures/mcp-history-compat/lib.rs::base_types_include_all_high_bits_and_full_types_are_exact` 检查基础数、packed 数、负数及旧 AST 差异。后者是历史选择 oracle，不单独替代当前 production 接线审查。运行结果：**待父集中验证**。

## 8. Voice 旧七字段与分页状态

源码：`business::voice::catalog::Entry` 只给出 username、timestamp、byte_len 与 opaque SourceRef；物理 source/chat_name_id/media_rowid/local_id 留在 adapter 证据及 `LegacyVoiceMessage`。`src/daemon/server.rs` 的 VoiceMessages 分支仅调用 `legacy_rows(&page)` 序列化，保留 `voices/count` 包装及旧七字段：username、source、chat_name_id、media_rowid、local_id、create_time、voice_data_bytes。

保证：维持 create_time 降序、canonical source 字典升序、local_id 降序、rowid 降序；维持 offset+limit 候选预算及时间闭区间。跨片/同片重复 local_id 不去重，NULL 与 0 不混淆。固定账号库存前后复核保持，缺片/未解密/额外分片失败，不成功返回已知子集。

分页：`Page.continuation` 使用局部 `PageContinuation::{Exhausted, MayHaveMore}`，与 messages 的同名类型同义但不引入跨业务模块依赖。数据源完整是成功前提，独立于分页是否结束。成功空页/短页为 Exhausted；满页未 probe，一律 MayHaveMore，即使恰好最后一页也不能确定结束，更不能声称确定 More。不额外查询，不给旧 wire 增加状态字段。

测试：`tests/fixtures/mcp-voice/tests.rs::{typed_previews_and_legacy_wire_golden_preserve_duplicate_ids, legacy_projection_rejects_foreign_or_changed_evidence}`；`tests/fixtures/mcp-voice-security/security_tests.rs::{security_pagination_matches_independent_oracle_for_ties_ranges_and_permutations, security_library_local_name_ids_and_binary_names_do_not_cross_contacts}`；`tests/fixtures/mcp-voice/lib.rs::adapter_tests::complete_inventory_and_undecrypted_shards_are_strict`；`src/business/voice/catalog/tests.rs::{complete_inventory_does_not_mean_pagination_is_exhausted, a_full_last_page_cannot_claim_exhaustion_without_a_probe}`。运行结果：**待父集中验证**。

例外：独立 voice fixture 使用真实业务/SQLite adapter/query 模块，但 DbCache 是公开接口替身，不代表真实解密集成；显式离线 Catalog 的完整库存由调用方保证。SourceRef 是预览证据而非唯一身份、音频读取授权或当前归属证明。库存前后相等不承诺跨库原子快照，Exhausted 不承诺未来不会新增语音。

## 验收范围与命名备注

`mcp_voice` 仍是历史命名的 daemon 查询门面，现行普通返回值已是业务 Page；`LegacyVoiceMessage`、`LegacyReadPolicy`、`legacy_wire_type` 是明确协议兼容所有者，不应仅因存在物理数字就误判普通业务重新依赖微信结构。引用消息预览的 `sender_label` 不是 author identity，协议字段仍可叫 sender，不能从 display 或 me 猜稳定身份。

本文件不新增功能、不重做任务工具，也不替代父代理的全量验收、密钥、发布/归档结论。所列测试需按当前共享工作区由父集中执行；只读核对和 formatter/diff 检查均不计作测试通过。
