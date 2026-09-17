# 普通消息读页边界

## 数据流

`history/search/new_messages` 的宿主只准备账号快照、解析请求与联系人显示上下文，随后调用 `Snapshot::history_page/search_page/new_messages_page`。这些方法位于 `adapters/wechat/messages/projection/pages.rs`，返回真正的 `business::messages::MessagePage`：`Vec<Message>`、完整性状态和快照绑定的 opaque 引用，不包含 source、path、table、local_type、rowid 或 JSON。

adapter 在分页前把每个候选解码为 `Message`，使用 `Page::select` 算法。物理分片排名、每片候选上限、全局分页、重复引用去重、搜索最终倒序和增量起点处理均在 adapter 内执行。宿主不读取 `RawMessage` 或对 `Candidate<Value>` 分页。

## 源完整性与续页状态

`MessagePage::completeness` 只表达源完整性，不代表分页已结束。独立的 `PageContinuation` 可供其他普通业务读页复用：`Exhausted` 表示候选读取均已耗尽且最终返回不足一页；`MayHaveMore` 表示无法确认是否还有下一页，不等同于已确认有下一页。

adapter 只读取每片 limit+offset 上限，不多取一条、不提高候选预算。任一候选读取返回满额，或最终返回满页，就保守标记 `MayHaveMore`；只有各候选读取均不足上限且最终不足一页（包括空页或 offset 超过结尾）才标记 `Exhausted`。即使跨片总数恰好凑满一页，也不宣称确定结束。重复 opaque 引用去重后不足一页，不能抵消候选读取尚未耗尽的不确定性。

该状态仅属于业务页，不出现在 CLI/HTTP/MCP wire 中；源不完整仍按既有规则报错，不通过续页状态伪装成正常部分结果。业务内存与 adapter 测试覆盖空页、不满页、满页、跨片相同内容的不同引用，以及重复引用去重后的保守判定。

## 独立的兼容投影

- `ReadPage::legacy` 是独立的 `LegacyPageProjection`，仅以选中消息的 opaque 引用查询；不参与普通业务排序、过滤或归属判断。
- 其可序列化消息材料仅恢复旧 wire 的 `local_id/source/type` 及未映射会话的诊断键。字段是私有的，宿主不能自行拼接物理定位规则；引用失效或属于其他快照时拒绝取投影。
- 历史 source 的斜杠规范化保持不变。未知会话在业务中仍为 `Conversation::Unmapped`，普通 username 仍为 null；旧搜索显示的表名只通过 `unmapped_chat_label()` 这一显式兼容诊断投影提供，不用来重建业务身份。
- `ReadPage::diagnostics` 只提供既有分片元数据所需的材料。宿主 `Prepared::history_metadata/global_metadata` 为明确的诊断投影：绑定账号缓存路径和模式，调用原 metadata 格式化；绝对路径仍受 `debug_source` 控制。分页和消息身份不依赖这些宿主路径。
- `find_shards` 用于诊断、兼容导出及统计读取，不参与 history/search 读页执行。

## 保持的兼容语义

- 历史的分片排名仍按该会话在各片的最新时间倒序，同值沿用快照源次序；排名在应用请求时间窗口前确定。
- 时间过滤仍包含 since 和 until 两个端点。每片读取 limit+offset 个候选，再按旧全局规则选择；历史最终正序，搜索保持原最终逆序，包括同秒次序。
- 去重仅作用于相同 opaque 引用；跨片或同片不同记录即使 local_id、时间、内容相同也不合并。
- 新消息仍从旧时间戳加一秒开始，先全局取最早的一页，再由 typed username/timestamp 计算已交付游标；不从展示 JSON 反推游标。
- 库存不依赖会话摘要集合。未知联系人/会话照常保留；schema、缺片、歧义和超限仍为严格错误，不伪造空结果或完整页。
- 全部候选仍在分页前解码并校验旧身份投影。即使坏候选最终不会被选入，也不会被延迟展示机制静默忽略。
- opaque 引用仍仅在创建它的快照存活期间有效；不是跨数据库同时刻事务、持久分页游标或可序列化凭据。

## 验证材料

`projection/pages/tests.rs` 覆盖 typed 页/兼容 wire 等价、跨片同秒排名、时间端点、无会话库存、未知身份诊断、分页外坏行、投影跨快照拒绝与过期、增量全局最早页测试。`message_read_tests.rs` 使用生产 typed 页；`message_source_tests.rs` 覆盖真实 `q_search` 链的排序、类型过滤、闭区间和缺失 session/Name2Id 情况。

执行命令与依赖见[测试说明](../../../tests/README.md)。
