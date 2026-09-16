# History Selection 兼容边界

当前 IPC、CLI 和 MCP 复用 daemon 中装配的新消息适配器与业务分页。下文保留上一阶段选择器的兼容核对记录，其中“新助手”和 `Selection` 已成为本目录的冻结参考实现，不再用于生产。当前实现的身份、完整性和读取预算见 [消息业务边界](../../../docs/business-messages.md)。


补充的消息身份与富字段组合回归已注册在 `src/daemon/query/message_source_tests.rs`，从仓库根运行 `cargo test --bin wx daemon::query::message_source_tests -- --nocapture`。它调用真实 history/new-messages 查询并使用临时合成加密库。

## 历史核对记录

- 旧 get_chat_history 接受 msg_types 多名称和 oldest_first。名称由旧 _resolve_msg_types 映射，file=app=49；空列表不筛选。
- 旧 _query_messages 在每分片应用闭区间和类型过滤，取 offset+limit 个候选；_page_ranked_entries 跨分片选页后按时间升序展示。最早页不是反转最新页。
- 原 q_history/query_messages 仅接受单一 msg_type、始终取最新页；push_msg_type_filter 按低 32 位筛选。新 q_history 的默认 None/false 仍走原选行与分页路径。
- 旧 Python 的 local_type IN 是完整值精确匹配，不包含高位变体。新助手保留 Rust base 类型能力：0..=u32::MAX 匹配低 32 位，范围外完整类型精确匹配；这是明确扩展，不声称旧实现已有。
- 同时间条目使用稳定排序，保留调用方分片顺序和 SQLite 返回顺序，不按 local_id 去重。SQLite 仅按时间排序，旧实现没有跨快照唯一游标保证。

## 参考实现语义

q_history 前十个参数不变，末尾增加 `msg_types: Option<&[i64]>`、`oldest_first: bool`。调用方使用 `msg_types.as_deref(), oldest_first`；默认使用 `None, false`。

1. q_history 查联系人之前拒绝 single 与非空 list 同时出现（包括同值）、超过 100 项列表、零 limit、offset+limit 溢出或超出 SQLite i64 范围、倒置时间区间。空列表不覆盖 single。类型接受完整 i64，不擅自丢弃未知类型而返回全部。
2. Selection 在循环前创建，先进每个分片 SQL 再合并分页；find_msg_shards、账号定位、group_nicknames、meta 构造保留。
3. 原 query_messages 和新 selection 都调用同一个 read_history_row/render_history_rows。get_content_bytes、ct 默认值、load_id2u、decompress_message、sender_label、fmt_content、URL 和 JSON 字段均保留，未复制第二套 mapper。
4. 新路径按原 shards 顺序合并后调用 selection.page。shard_hits 仍按每分片非空候选计数；windowed 对非空 types/oldest_first 更新。默认路径保留原最新分页行为。

助手不打开/关闭账号连接，不发现分片、不处理渲染异常补页，也不隐瞒 SQL/行映射错误。旧 Python 遇到渲染错误会记录 failures 并补取候选；现 Rust 映射错误向上传递，现有格式化边界无这种跳过语义。本次没有宣称覆盖这项输出差异。

## 验证

固定 oracle 保存迁移时生成的 288 组合成 SQLite 选行与分页结果。当前回归不再执行或携带旧 Python 源码。

`cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml -- --nocapture` 只核对测试目录中的旧选择器参考实现与 oracle，包括高位及有符号类型、原始字段和分页规则；它不再是生产查询的验收。当前生产读取由 `adapters::wechat::messages` 和业务分页执行，须以以下真进程测试、根工程 `mcp_readonly_runtime` 以及 `daemon::query::message_source_tests` 验证，不能把旧参考实现通过当成新通道通过。

真进程验证：先 `cargo build --bin wx`，将环境变量 `CARGO_BIN_EXE_wx` 设置为该绝对可执行路径，再运行 `cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml --features runtime --test runtime -- --nocapture`。

runtime.rs 复用 mcp-readonly-runtime 的合成 SQLCipher 账号及真实 IPC，不模拟 q_history。比较默认路径和新路径的完整消息 JSON，覆盖跨分片、重复 local_id、闭区间、最早/最新、offset、空页、参数错误先于联系人查询及源快照未修改。真实 MCP 测试是额外的协议层验证，不代替本测试。


依赖和人工审核点见[测试说明](../../README.md)。
