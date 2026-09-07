# History Selection 兼容边界

已按后续授权接入 query.rs 的 q_history，并最小提取 query_messages 的共享六列读取及行渲染 helper。IPC、CLI、协议由 main 接线；本任务未修改 search 或其他查询逻辑。

> 2026-09-07 文档核对：生产接线已存在，下文是该阶段实现与历史验收记录，不是仍待注册的清单。本次不执行 Cargo、不重建 oracle，也不把后续主仓通过数字填回本页。

补充的消息身份与富字段组合回归已注册在 `src/daemon/query/message_source_tests.rs`，从仓库根运行 `cargo test --bin wx daemon::query::message_source_tests -- --nocapture`。它调用真实 history/new-messages 查询并使用临时合成加密库；下方旧数字不包含这组后续覆盖，不能当作其运行结果。

## 已核对的差异

- 旧 get_chat_history 接受 msg_types 多名称和 oldest_first。名称由旧 _resolve_msg_types 映射，file=app=49；空列表不筛选。
- 旧 _query_messages 在每分片应用闭区间和类型过滤，取 offset+limit 个候选；_page_ranked_entries 跨分片选页后按时间升序展示。最早页不是反转最新页。
- 原 q_history/query_messages 仅接受单一 msg_type、始终取最新页；push_msg_type_filter 按低 32 位筛选。新 q_history 的默认 None/false 仍走原选行与分页路径。
- 旧 Python 的 local_type IN 是完整值精确匹配，不包含高位变体。新助手保留 Rust base 类型能力：0..=u32::MAX 匹配低 32 位，范围外完整类型精确匹配；这是明确扩展，不声称旧实现已有。
- 同时间条目使用稳定排序，保留调用方分片顺序和 SQLite 返回顺序，不按 local_id 去重。SQLite 仅按时间排序，旧实现没有跨快照唯一游标保证。

## 已完成接线

q_history 前十个参数不变，末尾增加 `msg_types: Option<&[i64]>`、`oldest_first: bool`。调用方使用 `msg_types.as_deref(), oldest_first`；旧调用使用 `None, false`。

1. q_history 查联系人之前拒绝 single 与非空 list 同时出现（包括同值）、超过 100 项列表、零 limit、offset+limit 溢出或超出 SQLite i64 范围、倒置时间区间。空列表不覆盖 single。类型接受完整 i64，不擅自丢弃未知类型而返回全部。
2. Selection 在循环前创建，先进每个分片 SQL 再合并分页；find_msg_shards、账号定位、group_nicknames、meta 构造保留。
3. 原 query_messages 和新 selection 都调用同一个 read_history_row/render_history_rows。get_content_bytes、ct 默认值、load_id2u、decompress_message、sender_label、fmt_content、URL 和 JSON 字段均保留，未复制第二套 mapper。
4. 新路径按原 shards 顺序合并后调用 selection.page。shard_hits 仍按每分片非空候选计数；windowed 对非空 types/oldest_first 更新。默认路径保留原最新分页行为。

助手不打开/关闭账号连接，不发现分片、不处理渲染异常补页，也不隐瞒 SQL/行映射错误。旧 Python 遇到渲染错误会记录 failures 并补取候选；现 Rust 映射错误向上传递，现有格式化边界无这种跳过语义。本次没有宣称覆盖这项输出差异。

## 验证

`python tests/fixtures/mcp-history-compat/generate_oracle.py` 仅 AST 抽取旧纯函数，不导入旧服务、不读账号。生成 288 组真实 SQLite 选行与旧分页结果，oracle 内记录旧源码 SHA256。

`cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml -- --nocapture` 编译真实新助手，核对 oracle、高位及有符号类型、原始 sender/content/压缩字段、分页溢出、标识符和错误传播。无需主程序接线。

真进程验证：先 `cargo build --bin wx`，将环境变量 `CARGO_BIN_EXE_wx` 设置为该绝对可执行路径，再运行 `cargo test --manifest-path tests/fixtures/mcp-history-compat/Cargo.toml --features runtime --test runtime -- --nocapture`。

runtime.rs 复用 mcp-readonly-runtime 的合成 SQLCipher 账号及真实 IPC，不模拟 q_history。比较默认路径和新路径的完整消息 JSON，覆盖跨分片、重复 local_id、闭区间、最早/最新、offset、空页、参数错误先于联系人查询及源快照未修改。main 的真 MCP 测试是额外的协议层验证，不代替本测试。

本轮已实跑：既有 query 单测 103 项通过；本 fixture 助手测试 4 项（含 288 组 oracle）及真 IPC 测试 1 项通过。日志位于 C:/CodexLocal/history-query-unit.log、history-query-runtime.log、mcp-history-compat-final.log。主程序编译曾出现其他任务新增 raw_db_keys 未使用警告，不在本任务范围内。
