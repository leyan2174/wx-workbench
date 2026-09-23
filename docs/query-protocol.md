# CLI 与 MCP 查询协议

本文描述当前查询与结构化详情入口。所有查询限定在宿主配置的账号内，复用 daemon 查询业务；“只读”不表示完全不产生内部解密缓存，而是不会替用户发布附件、修改微信记录或启动导出任务。HTTP 的独立契约见[本地 HTTP API](http-api.md)，入口覆盖见[能力矩阵](capability-matrix.md)。

## CLI 查询与详情

以下命令在顶层注册，均支持 `--json`，默认输出 YAML，保留 daemon 返回的结构与业务状态。

```text
wx tags [--json]
wx tag-members <TAG_NAME> [--json]
wx decode-refer <CHAT> <LOCAL_ID> [CREATE_TIME] [--json]
wx decode-file-message <CHAT> <LOCAL_ID> [CREATE_TIME] [--json]
wx decode-record-item <CHAT> <LOCAL_ID> <ITEM_INDEX> [CREATE_TIME] [--json]
wx voice-messages <CHAT> [-n|--limit N] [--offset N]
                  [--since DATE_TIME] [--until DATE_TIME] [--json]
```

- 目标不得为空白，最多 4096 个字符；保留用户输入，不暗自改写标签名或会话标识。
- 三种详情命令要求 `LOCAL_ID > 0`，记录 `ITEM_INDEX >= 0`。`CREATE_TIME` 是 i64 Unix 秒，默认 `0` 表示不按时间过滤；存在多条候选时仍失败，不取第一条。既有 `decode-transfer`、`decode-location` 同样保留时间戳 `0` 的 wildcard 语义。
- `voice-messages` 的 limit 默认 20，范围 1..500；offset 默认 0，范围 0..1000000。`--since` / `--until` 复用 CLI 日期解析，不接受 Unix 秒字符串；反向区间拒绝。
- `tags` 返回标签目录及关联计数；`tag-members` 查询唯一匹配标签，精确优先再模糊匹配，不能据目录计数假定已取得成员列表。CLI 和 MCP 的单标签查询目标在 trim 后必须非空，daemon 在读取标签源前再次校验；这不改变标签列表对源数据空标签名的兼容处理，也不禁止联系人列表的空筛选。
- 详情原样保留引用、附件状态、元数据与现有本地引用，不下载或创建附件。语音目录返回 `voices/count`，不读取音频正文；没有 `has_more`，一整页只能表示“可能还有记录”。
- 查询客户端与根分发复用业务错误投影：失败非零退出，typed 身份歧义及既有结构化定位歧义保留退出码 2；缺分片、不可用和超限不能伪装成空列表。不存在的资源状态或未知大小不补成成功路径或零字节。

实现入口：[CLI 接线](../src/cli/mod.rs)、[参数和映射](../src/cli/query_details.rs)、[查询客户端](../src/service/query_client.rs)。

## MCP 工具

默认工具集有 23 项，22 项查询/元数据工具及一个受控图片写出工具 `decode_image`。后台任务工具仅在宿主显式启用后增加，不算入这个数量。会话、认证与图片发布边界见[宿主协议](../src/mcp/PROTOCOL.md)；当前只读工具清单和参数以本页及[注册源码](../src/mcp/protocol.rs)为准。

下表列出全部默认工具；`*` 为必填，未标注的参数可省略。省略 limit 的缺省值来自查询 Request，schema 允许的范围与缺省值是不同概念。

| 工具 | 参数 | limit 默认值 / 主要结果 |
| --- | --- | --- |
| `get_recent_sessions` | limit | 20；会话摘要 |
| `get_contacts` | query, limit | 50；联系人，limit 允许 0..500 |
| `get_chat_history` | chat_name*, limit, offset, since, until, msg_type, msg_types, oldest_first, with_meta | 50；消息页，跨分片过滤合并 |
| `search_messages` | keyword*, chats, limit, offset, since, until, msg_type, with_meta | 20；跨会话搜索结果 |
| `decode_transfer` | chat_name*, local_id*, create_time | 转账结构，不执行转账 |
| `decode_location` | chat_name*, local_id*, create_time | 位置结构 |
| `get_new_messages` | 无 | 会话摘要轮询，不是消息增量流 |
| `get_chat_images` | chat_name*, limit, offset, since, until | 20；图片资源元数据，不执行解码 |
| `get_contact_tags` | 无 | tags、total_tags、total_associations |
| `get_tag_members` | tag_name* | 唯一标签的成员 |
| `decode_refer` | chat_name*, local_id*, create_time | 结构化引用 |
| `get_voice_messages` | chat_name*, limit, offset, since, until | 20；voices、count |
| `decode_file_message` | chat_name*, local_id*, create_time | 文件元数据、状态和本地引用 |
| `decode_record_item` | chat_name*, local_id*, item_index*, create_time | 完整记录 datalist 的零基条目 |
| `decode_image` | chat_name*, local_id*, create_time | 宿主受控图片写出；不是只读查询 |
| `get_unread_messages` | limit, filter, with_meta | 20；未读会话摘要，不标记已读 |
| `get_chat_members` | chat_name* | 成员及 membership_complete / membership_source |
| `get_chat_stats` | chat_name*, since, until, with_meta | 会话统计，不按当前结果页推算 |
| `get_favorites` | limit, fav_type, query | 50；items、count、has_more |
| `get_biz_articles` | limit, account, since, until, unread | 50；文章及 partial / issues / source_unfinished / has_more |
| `get_sns_feed` | limit, user, since, until | 20；本地朋友圈及覆盖元数据 |
| `search_sns` | keyword*, limit, user, since, until | 20；本地朋友圈文本搜索 |
| `get_sns_notifications` | limit, since, until, include_read | 50；本地互动，默认仅未读 |

通常 limit 为 1..500，offset 默认为 0；history、图片和语音目录 offset 上限 1000000。search 的 offset 上限 9999，且 `offset + limit <= 10000`：适配层扩大底层 Search 的候选数量，再对全局结果切片，不是新增 IPC Search.offset。其余八个只读工具没有 offset；成员和统计也没有 limit，不虚构后端不存在的分页。

### 参数约束

- MCP 的 `since/until` 是 i64 Unix 秒，拒绝反向区间，不解析 CLI 日期字符串；按各业务既有时间字段和包含端点执行。
- 字符串最多 4096 字符，数组最多 100 项；chat_name、keyword、account、user 不得为空白，chats 内不能有空白目标。favorites.query 允许空串表示无文本过滤。
- 未读 filter 为数组，允许 private/group/official/official_account/folded/fold/all；空数组或包含 all 沿用全部类型语义。fav_type 是非负 i64，不把已知收藏类型枚举当作完整类型集合。
- history 的 msg_types 为类型名称数组，允许空数组或 null 表示不增加多类型过滤；非空解析结果与 msg_type 冲突时拒绝。msg_type 是数值 wire selector。search 不支持 msg_types 或 oldest_first。
- 五种只读详情的 create_time 省略或为 0 时不按时间过滤；文件/记录的 local_id 必须为正，记录索引非负。旧 transfer/location/refer 的 schema 仍接受 i64 local_id，不应把它们描述成与新附件校验完全相同。
- 所有查询工具拒绝未知字段，不接受任意命令、账号切换、宿主路径注入。`debug_source` 不公开，传 false/null 也拒绝；daemon 还在打开账号前拒绝 raw Request 中的 debug_source=true。普通 with_meta 不等于原始分片路径授权。

### 结果与错误

参数错误返回 JSON-RPC `-32602`，不进入业务分发。业务失败走 `isError=true` 的脱敏结果，不透出底层数据库错误链。普通数据中的 `partial:true`、嵌套 meta、issues、has_more 等覆盖信息保留，不擅自改成顶层业务失败，也不能丢弃后宣称完整成功。

共享查询边界 `query_response` 保留 `business::contacts::Error::Ambiguous` 与 `business::messages::Error::Ambiguous` 的错误类型，包括标签和 SNS 作者解析附加的上下文，不先转成字符串再分类。此类失败的查询响应携带 `status=ambiguous`、`error_code=ambiguous_identity`、`exit_code=2`，属于明确的身份歧义拒绝。MCP 依既有 Refused 投影返回 `isError=true` 和 `Business request refused`，不是通用 `Query failed`；HTTP 对应安全 409 `query_ambiguous`。这些字段描述共享查询响应，不表示 MCP 向模型原样透出内部错误载荷。

其他业务或来源失败继续按实际错误类别处理；即使错误文本包含 ambiguous 等字样，也不能据字符串猜测为身份歧义。普通查询的孤立 exit_code=2 不自动等同于 typed ambiguity；五类结构化详情原有的退出码歧义契约另行保留。

群成员的 `observed_senders` 只表示观察到的发送者，不代表完整成员名册。公众号时间按接收时间筛选；unread 表示未读公众号交集中的每公众号最新一篇，不是文章逐篇已读状态。SNS 只读本地缓存，保留 local_cache_only、scan_truncated、unreadable、author_conflicts 等字段；include_read 不改变已读状态。

`get_new_messages` 保持无参数、会话内游标的旧契约：首次返回未读会话摘要，之后返回摘要发生变化的会话。它不是 CLI `new-messages`，也不是持续 `monitor`。

## 跨入口时间与类型

| 项目 | CLI | MCP | HTTP / Web |
| --- | --- | --- | --- |
| 查询日期输入 | 本地 YYYY-MM-DD 或带空格的 HH:MM[:SS]；日期上界到 23:59:59 | i64 Unix 秒 | HTTP 非负 Unix 秒；Web datetime-local 转成 Unix 秒 |
| 详情时间 | 省略/0 为 wildcard | 五种只读详情省略/0 为 wildcard | create_time 必填且 >0，精确定位 |
| 搜索偏移 | 既有 search 无 offset | 候选窗口内全局切片 | search 不支持 offset |
| 多类型与最早页 | history 支持 | history 支持 | 选定会话的 history 支持；本页筛选不等于服务端过滤 |
| 原始来源调试 | 既有全局 --debug-source | 不公开 | 部分本地 HTTP 查询有 debug_source；可能包含私有路径 |

共享类型过滤将 `link` 与 `file` 都投影为宽泛的应用消息类型 49，不能承诺精确区分链接、文件和其他应用消息。MCP 的 msg_types 名称经大小写归一化解析；HTTP 还接受非负数值 wire selector。消息实际结构与具体详情应从返回内容判断，不能仅根据筛选标签命名。实现见[时间解析](../src/service/time.rs)、[类型投影](../src/service/message_filter.rs)与[Request](../src/ipc.rs)。

## 验证边界

CLI 解析/映射测试、MCP schema/真实业务分派测试和 HTTP 路由测试是不同层次的证据，存在测试源码不代表已经执行。跨入口与浏览器检查使用[合成账号夹具](../tests/fixtures/g1-query/README.md)；不能由工具列表、模拟响应或本页统计推断全部业务正确。原始 SILK 与关联 manifest 边界不变，不提供语音识别、音频转码或转录写回。
