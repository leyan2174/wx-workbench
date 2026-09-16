# 导出计划 CSV 测试

生产路径：`wx toolkit export-chats-native OUTPUT --from-plan-csv PLAN.csv [--plan-mode blacklist|whitelist]`。参数由公开 CLI 封送给 daemon 侧导出编排。默认不带计划时原 users/date/incremental/dry-run 路径保留，不生成计划、不串联 ASR。

## 源码对照

选择语义最初从 wechat-decrypt 的聊天导出流程迁移；`oracle.json` 是迁移时固定的合成契约，不再绑定仓库内的 Python 源码。

- 默认 blacklist：只有 trim 后的 `0` 跳过，包括空白在内的其他标记均选择。whitelist：只有 trim 后的 `1` 选择。`yes` 等标记沿用旧语义，不擅自解释为布尔值或报错。
- username trim 后精确匹配，区分大小写；显示名、序号、类型、数量、大小及时间统计列不用于账号绑定。重复 username 即使全部不选，也整批拒绝。
- 输出遵循 CSV 行顺序。users 参数或 WECHAT_EXPORT_USERS 先限制会话集合，CSV 中选中的集合外 username 整批报错，而不是默默取交集或导出全部。未选中的失效 username 可留在 CSV。
- 只要求 username 列，与旧消费者一致；缺少 export 列按空标记处理。兼容原生 chat-plan-native 的 UTF-8 BOM、CRLF、12 列及标准 CSV 引号/逗号/换行。
- 加强损坏输入拒绝：空/重复表头、无身份、UTF-8 无效、字段数不等、错位或未闭合引号、username 控制字符、过长身份、重复的后端会话身份。16 MiB、100000 行上限明确报错。csv crate 负责分列/转义/编码/列数；小型引号预检仅弥补其宽容语法，不手写 split。
- 参数 plan-mode 单独出现或值无效由 Clap 拒绝。计划读取先于 IPC；全部身份校验先于索引创建。空选择集成功返回且不创建输出目录；dry-run 不写计划、导出文件、锁或索引。

## 验证方式

1. `cargo test --manifest-path tests/fixtures/plan-selection/Cargo.toml --lib -- --nocapture`：真实 helper 对照固定 oracle 和损坏/超限输入。
2. `cargo test --bin wx export_chats -- --nocapture`：保留时间范围回归，直接验证原生 render_plan_csv 输出与 Args。
3. 先 `cargo build --bin wx`，将 CARGO_BIN_EXE_wx 设为构建产物绝对路径，再 `cargo test --manifest-path tests/fixtures/plan-selection/Cargo.toml --features runtime --test runtime -- --nocapture`。

runtime 复用现有合成 SQLCipher 工具与 daemon 生命周期，不模拟目标 CLI、选择器或导出 JSON。两名联系人拥有相同显示名，验证名单顺序、blacklist/whitelist、用户和环境过滤、错误计划、空选择、实际导出 username、日期闭区间和增量追加。源快照与计划字节核对不变。

不声称旧 Python 的宽容损坏 CSV 行为完全兼容；这些输入是明确安全收紧。CSV 中的展示/统计列不被当作可靠的导出时间条件，实际时间条件仍由命令行 start/end 决定。

运行环境和人工审核点见[测试说明](../../README.md)。固定 oracle 只作为迁移契约，不由生产实现重新生成。
