# 本地 HTTP 查询 API

本页描述 `wx web` 当前查询与消息详情接口，不是任意 daemon Request 的 HTTP 代理。服务器固定当前账号；请求不能指定 runtime、配置、密钥或输出根。Web UI、HTTP、MCP 的覆盖范围见[能力矩阵](capability-matrix.md)，CLI/MCP 参数见[查询协议](query-protocol.md)。

## 认证与公共规则

- 下表接口均为 GET，沿用 loopback、Host/Origin 检查和 `x-wx-token` 认证；不要将服务暴露到不可信网络。写入任务和图片解码仍使用原有单独接口与授权，不由查询触发。
- 新查询接口使用固定参数白名单。未知、不适用于该路由、重复的键均拒绝；`type` 是 `msg_type` 别名，二者同时出现也拒绝。可传 `source=wechat`，不支持其他源或账号切换。
- 布尔值为 `true` / `false`；数组为逗号分隔的查询值，需 URL 编码。不要把 JSON 数组直接塞入查询字符串。
- 有 limit 的新接口默认 200，范围 1..2000；语音目录例外，范围 1..500。支持 offset 的接口默认 0，上限 1000000。成员和统计没有分页参数。
- `since/until` 是非负 i64 Unix 秒，`since <= until`；不会自动把某个秒值扩展为整天。Web datetime-local 使用浏览器本地时间转秒；CLI 本地日期上界包含整天的规则不适用于 HTTP。
- 目标文本非空白、最长 256 字节且不含控制字符；收藏 query 如不筛选应省略，传空串也拒绝，这与 MCP 不同。chats 最多 100 项；history 的 msg_types 为非空、最多 64 项的列表。
- 成功响应直接是 daemon 业务数据，没有新增 `data` 包裹。原有 history 展示装饰保留；其他数据中的 meta、partial、issues、has_more 等不因 HTTP 适配被丢弃。

本节严格白名单规则针对下面的新查询与扩展 history。既有 contacts/sessions/tags/tag-members/images 使用原 Filter 契约，不据此宣称这些旧入口已统一成新 schema。

## 路由与参数

`*` 表示必填；未列出的参数不受支持。

| 路径 | 参数 | 主要响应与范围 |
| --- | --- | --- |
| `/api/history` | chat*, limit, offset, since, until, msg_type/type, msg_types, oldest_first, with_meta, debug_source | messages、username、chat 及业务元数据；选定会话的跨分片历史 |
| `/api/search` | keyword*, chats, limit, since, until, msg_type/type, with_meta, debug_source | keyword、count、results、meta；没有 offset、msg_types、oldest_first |
| `/api/unread` | limit, filter, with_meta, debug_source | sessions 及汇总计数/元数据；未读会话摘要，不修改已读状态 |
| `/api/members` | chat* | members、count、membership_complete、membership_source |
| `/api/stats` | chat*, since, until, with_meta, debug_source | 原始会话统计，不是前端当前页聚合 |
| `/api/favorites` | limit, fav_type, query | items、count、has_more；只查询，不导出或下载 |
| `/api/articles` | limit, account, since, until, unread | articles、count、partial、issues、source_unfinished、has_more |
| `/api/sns-feed` | limit, since, until, user | posts、total、resolved_user、meta；本地缓存 |
| `/api/sns-search` | keyword*, limit, since, until, user | keyword、posts、total、meta；本地文本搜索 |
| `/api/sns-notifications` | limit, since, until, include_read | notifications、total；默认未读，不改变通知状态 |
| `/api/voice-messages` | chat*, limit, offset, since, until | voices、count；不读取音频正文，没有 has_more |
| `/api/decode-transfer` | chat*, local_id*, create_time* | 原结构化转账详情 |
| `/api/decode-location` | chat*, local_id*, create_time* | 原结构化位置详情 |
| `/api/decode-refer` | chat*, local_id*, create_time* | 原结构化引用详情 |
| `/api/decode-file-message` | chat*, local_id*, create_time* | 文件元数据、状态和本地引用 |
| `/api/decode-record-item` | chat*, local_id*, create_time*, item_index* | 零基记录条目、元数据和本地引用 |

无 chat 的 `/api/history` 继续使用启动监控范围，仅接受原 limit/offset/since（及 source、空 chat）参数；until、类型、顺序等扩展条件必须先选会话，否则拒绝，不能悄悄当作监控筛选。

history 的 msg_type 与 msg_types 互斥。类型允许 text/image/voice/video/sticker/location/link/file/call/system 或非负 i64 wire selector。**link 与 file 都是宽泛应用消息类型 49，不保证精确分类。** unread.filter 允许 private/group/official/folded/all，非空且最多 5 项，不接受 MCP 的 official_account/fold 别名。

fav_type 是非负整数，不限定为界面所列的少数类型。articles.account 是公众号显示名筛选，不是账号切换；时间为接收时间，unread 是未读公众号交集中的每公众号最新一篇。SNS 覆盖和截断字段应保留；空列表不能证明账号没有历史数据。

五个详情接口都要求 `local_id > 0`、`create_time > 0`，记录要求 `item_index >= 0`。HTTP 不使用 CLI/MCP 的 create_time=0 wildcard。必须沿用所选消息的精确会话与时间戳；不接受任意文件路径，不自动下载、复制或导出附件。

## 已有查询入口

`GET /api/contacts`、`/api/sessions`、`/api/tags`、`/api/tag-members`、`/api/images` 继续存在。标签列表支持可选 `name` 筛选；标签成员使用必填 `name`，不是 `tag_name`。Web 已有标签目录与成员视图，无需以本次资料查询面板替代。

图片列表和缓存读取、受控解码是不同接口；`POST /api/images/{id}/decode` 不是只读查询。任务接口和事件流也不代表所有业务均可从 HTTP 执行。相关执行边界见[后台任务](daemon-tasks.md)与[严格媒体边界](strict-media-host-boundary.md)。

## 错误与不完整结果

| HTTP 状态 | 含义 |
| --- | --- |
| 400 | 参数无效、未知字段或不支持的组合 |
| 401 / 403 | 认证或本地来源校验失败 |
| 409 | 顶层业务 partial，或明确的查询歧义；两者不能混为同一状态 |
| 422 | 未被明确歧义规则识别的业务 refused / failure；身份歧义优先返回 409 |
| 429 | 查询繁忙或并发限制 |
| 503 | 当前账号查询不可用 |

一般错误返回固定公开 `error`，不返回底层错误链。共享查询边界保留 contacts/messages 的 typed Ambiguous，在查询响应中标记 `status=ambiguous`、`error_code=ambiguous_identity`、`exit_code=2`；标签及 SNS 作者解析的错误上下文不丢弃该类型。这类查询歧义不限于五种详情，HTTP 优先按明确顶层 status=ambiguous 投影为 409，不被通用 refused 的 422 覆盖。五种结构化详情原有的 exit_code=2 同样使用专用 `query_ambiguous` 投影：

```json
{"error":"Query identity is ambiguous; specify an exact conversation and message","code":"query_ambiguous","status":"ambiguous","exit_code":2}
```

其中 `error` 的具体文字以服务常量为准，调用方应判断 `code/status`，不依赖文本。其他普通查询只有 exit_code=2、没有明确歧义状态时，不自动视为身份歧义；其他业务或来源错误也不凭字符串推测为 ambiguity。成功业务数据里的 `partial:true` 或嵌套 meta/issues 并不等于顶层 business partial，仍保留原数据；客户端必须展示不完整状态，而不是改为空或完整成功。

## Web 界面与实现

资料查询面板提供上述搜索、未读、成员、统计、收藏、文章、朋友圈和语音目录。聊天“记录范围”向 history 提交时间、多类型和顺序；本页文字/类型筛选仍仅改变已加载页的可见内容。消息详情使用所选消息身份调用五种详情接口，记录条目由零基索引选择。语音下一页按满页推测，仅标为“可能还有记录”，不宣称服务返回 has_more。

控件通过上述专门路由提交请求并呈现结果状态。HTTP 模拟响应测试只证明传输和参数接线，不能替代[合成数据库与浏览器检查](../tests/fixtures/g1-query/README.md)。

实现链路：[HTTP 路由](../src/web/mod.rs) → [参数解析](../src/web/read_queries.rs) → [固定账号 Call 与校验](../src/service/web.rs) → [daemon Request 映射](../src/daemon/web_service/read_queries.rs) → [查询分派](../src/daemon/server.rs)。界面在 [HTML](../src/web/assets/index.html) 与 [JS](../src/web/assets/app.js)。
