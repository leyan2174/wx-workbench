# History Selection 兼容边界

IPC、CLI 和 MCP 复用 daemon 中装配的消息适配器与业务分页。本目录的 `Selection` 是冻结的测试参考实现，不用于生产。生产实现的身份、完整性和读取预算见[消息业务边界](../../../docs/business-messages.md)。

消息身份与富字段组合回归注册在 `src/daemon/query/message_source_tests.rs`，从仓库根运行 `cargo test --bin wx daemon::query::message_source_tests -- --nocapture`。它调用真实 history/new-messages 查询并使用临时合成加密库。

## 兼容核对边界

- wechat-decrypt 的 get_chat_history 参考接口接受 msg_types 多名称和 oldest_first；_resolve_msg_types 映射 file=app=49，空列表不筛选。
- 参考分页在每分片应用闭区间和类型过滤，取 offset+limit 个候选；跨分片选页后按时间升序展示。最早页不是反转最新页。
- Python 参考接口的 local_type IN 是完整值精确匹配，不包含高位变体。冻结 Rust 选择器的 0..=u32::MAX 匹配低 32 位，范围外完整类型精确匹配；两者语义不同。
- 同时间条目使用稳定排序，保留调用方分片顺序和 SQLite 返回顺序，不按 local_id 去重。SQLite 仅按时间排序，不保证跨快照唯一游标。

## 参考实现语义

参考 q_history 的末尾参数为 `msg_types: Option<&[i64]>`、`oldest_first: bool`。调用方使用 `msg_types.as_deref(), oldest_first`；默认使用 `None, false`。

1. q_history 查联系人之前拒绝 single 与非空 list 同时出现（包括同值）、超过 100 项列表、零 limit、offset+limit 溢出或超出 SQLite i64 范围、倒置时间区间。空列表不覆盖 single。类型接受完整 i64，不擅自丢弃未知类型而返回全部。
2. Selection 在循环前创建，先进每个分片 SQL 再合并分页；find_msg_shards、账号定位、group_nicknames、meta 构造保留。
3. query_messages 和 selection 都调用同一个 read_history_row/render_history_rows。get_content_bytes、ct 默认值、load_id2u、decompress_message、sender_label、fmt_content、URL 和 JSON 字段共用同一 mapper。
4. 选择器路径按调用方 shards 顺序合并后调用 selection.page。shard_hits 仍按每分片非空候选计数；windowed 对非空 types/oldest_first 更新。默认路径取最新页。

选择器不打开/关闭账号连接，不发现分片、不处理渲染异常补页，也不隐瞒 SQL/行映射错误。Python 参考实现遇到渲染错误会记录 failures 并补取候选；Rust 映射错误向上传递，格式化边界不跳过这些错误。固定 oracle 不证明这项错误输出等价。

## 验证

固定 oracle 保存 288 组合成 SQLite 选行与分页结果，来源于 Python 参考实现。回归不执行或携带该参考源码。

`cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml -- --nocapture` 只核对测试目录中的冻结选择器参考实现与 oracle，包括高位及有符号类型、原始字段和分页规则；它不是生产查询的验收。当前生产读取由 `adapters::wechat::messages` 和业务分页执行，须以以下真进程测试、根工程 `mcp_readonly_runtime` 以及 `daemon::query::message_source_tests` 验证，不能把参考实现通过当成生产查询通过。

真进程验证：先 `cargo build --bin wx`，将环境变量 `CARGO_BIN_EXE_wx` 设置为该绝对可执行路径，再运行 `cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml --features runtime --test runtime -- --nocapture`。

runtime.rs 复用 mcp-readonly-runtime 的合成 SQLCipher 账号及真实 IPC，不模拟 q_history。比较默认路径和显式选择器路径的完整消息 JSON，覆盖跨分片、重复 local_id、闭区间、最早/最新、offset、空页、参数错误先于联系人查询及源快照未修改。真实 MCP 测试是额外的协议层验证，不代替本测试。


依赖和人工审核点见[测试说明](../../README.md)。
