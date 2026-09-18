# 当前能力矩阵

本矩阵描述当前源码的公开入口和业务边界，不是工具数量统计或运行验收证书。导出、受控媒体、配置治理和增量能力仍有跨入口缺口。既有 CLI 操作继续存在，但不能因有一个任务提交入口，就推断该领域全部参数、输出和交互已覆盖。

## 判定方式

- **已接通**：源码可追到入口、参数校验、真实 Request/业务分发与结果呈现；不等于本次已跑通真实账号或浏览器。
- **部分**：只有相关子集或不同契约，不能替代本行完整业务。
- **无对应入口**：当前公开接口没有该专门能力；不等于底层业务不存在。
- **专属**：合理的传输或宿主差异，不要求虚构对等入口。

各入口通过合成账号和稳定身份核对正常、空、歧义、坏参、缺分片、跨账号及不完整结果；测试覆盖范围见[真实查询夹具](../tests/fixtures/g1-query/README.md)。不使用真实账号数据作为验收前提。

## 查询与详情

| 能力 | CLI | MCP | HTTP | Web 界面 |
| --- | --- | --- | --- | --- |
| 会话、联系人 | sessions / contacts | get_recent_sessions / get_contacts | /api/sessions / contacts | 已有列表 |
| 标签目录、标签成员 | tags / tag-members | get_contact_tags / get_tag_members | /api/tags / tag-members | 已有标签与成员入口 |
| 历史查询 | history | get_chat_history | /api/history | 会话历史与服务端“记录范围”已接通 |
| 跨会话消息搜索 | search | search_messages | /api/search | 资料查询中的消息搜索已接通 |
| 未读会话摘要 | unread | get_unread_messages | /api/unread | 查询面板已接通；不只是列表未读徽标 |
| 群成员 | members | get_chat_members | /api/members | 查询面板已接通 |
| 会话统计 | stats | get_chat_stats | /api/stats | 查询面板已接通；不是本页聚合 |
| 收藏查询 | favorites | get_favorites | /api/favorites | 查询面板已接通 |
| 公众号文章 | biz-articles | get_biz_articles | /api/articles | 查询面板已接通 |
| 本地朋友圈 | sns-feed | get_sns_feed | /api/sns-feed | 查询面板已接通 |
| 朋友圈搜索 | sns-search | search_sns | /api/sns-search | 查询面板已接通 |
| 朋友圈互动 | sns-notifications | get_sns_notifications | /api/sns-notifications | 查询面板已接通 |
| 只读语音目录 | voice-messages | get_voice_messages | /api/voice-messages | 查询面板及偏移翻页已接通 |
| 转账、位置详情 | decode-transfer / decode-location | decode_transfer / decode_location | /api/decode-transfer / decode-location | 消息详情已接通 |
| 引用详情 | decode-refer | decode_refer | /api/decode-refer | 消息详情已接通 |
| 文件详情 | decode-file-message | decode_file_message | /api/decode-file-message | 消息详情已接通 |
| 合并记录条目详情 | decode-record-item | decode_record_item | /api/decode-record-item | 消息详情与零基索引已接通 |
| 图片元数据与预览 | attachments；extract 为另行写出 | get_chat_images；decode_image 为受控写出 | /api/images；单图读取和 POST 解码另分权限 | 已有图片展示与受控预览；并非全部离线媒体操作 |

### 不能抹平的差异

| 项目 | 当前契约 |
| --- | --- |
| 时间 | CLI 日期为本地时间，日期 until 包含整天；MCP 是 i64 Unix 秒，HTTP 是非负 Unix 秒；Web 本地时间控件转换后提交秒值。 |
| 详情身份 | CLI/MCP 五种只读详情保留 create_time 省略/0 wildcard；HTTP 必须正 local_id、正 create_time，不能用 0 绕开时间定位。 |
| 分页 | CLI/MCP 语音默认 20，HTTP 默认 200；语音均最多 500，offset 最多 1000000。其他工具按各自 schema，不能用一组全局默认值替代。 |
| 搜索 | CLI/HTTP search 没有 offset；MCP 在 offset+limit<=10000 的候选窗口内切片。三端 search 均没有 history 的多类型/oldest_first 契约。 |
| 消息类型 | link/file 都走宽泛 legacy 应用消息 49，不保证精确分类；history 多类型与单类型的空值/冲突规则按入口分别校验。 |
| Web 筛选 | 原文字/类型筛选只改变本页；“记录范围”重新查询服务端；资料查询搜索是另一个后端接口。 |
| 元数据 | MCP 支持指定工具的 with_meta，不公开 debug_source；CLI 和部分本地 HTTP 有调试来源参数，不应把私有路径带入公开报告。 |
| 歧义拒绝 | contacts/messages 的 typed Ambiguous 保留为 status=ambiguous、error_code=ambiguous_identity、exit_code=2；CLI 非零退出，MCP 返回 Business request refused，HTTP 返回安全 409 query_ambiguous。不凭其他失败文本猜测身份歧义。 |
| 结果完整性 | 成员 observed_senders、SNS local_cache_only/scan_truncated、文章 partial/issues/source_unfinished、未知语音大小都不能当作完整确定结果。 |

完整参数、错误及默认值见[CLI/MCP 查询协议](query-protocol.md)、[HTTP API](http-api.md)。接口覆盖不等同于所有规模与数据变体均已验证。

## 执行类能力与剩余范围

| 分组 | 当前已有 | 尚未完成的跨入口范围 |
| --- | --- | --- |
| 导出与计划 | CLI 单聊/批量、delta、计划、表情、SNS 相册/快照/归档；目录任务支持 dry-run、媒体预算与产物交付；单会话历史任务支持 Markdown/TXT/JSON/YAML；会话摘要目录计划支持生成、不可变审阅、黑白名单及新目录原始 JSON 执行，三入口共享任务和文件 | 增量更新、宿主离线来源及其他导出仍未完整跨入口公开。计划目录不等于消息全集，审阅不等于授权审批。目录任务 JSON/CSV/HTML 不替代所有 CLI 导出；任务成功也不等于文件已被用户下载。 |
| 受控媒体与原始语音 | CLI voices 与完整聊天导出保留 SILK/关联 manifest；export_voices 任务共享原始 SILK/无路径证据、全账号或单会话选择及产物读取；MCP 受控图片，Web 图片预览，任务图片/SNS 子集 | 原始语音任务固定新目录，尚不覆盖 CLI 的已有目录 overwrite；CLI 离线图片/视频能力也未全部对等公开。 |
| 配置治理与诊断 | CLI setup、cleanup、progress/status/latency、账号和材料命令；任务仅有明确注册的初始化/材料/解密子集 | 不具备全量的 MCP/HTTP/Web setup 审阅/应用、清理计划及精确删除、诊断与高级账号材料入口。 |
| 增量语义与交互 | CLI new-messages 与 monitor；MCP get_new_messages 会话摘要轮询；Web 监控/事件/任务状态 | 三者不是同一语义；尚不能宣称全部增量参数、游标与满额行为、幂等提交、未知结果恢复及取消/重连交互已对齐验收。 |

持久任务当前注册十一种公开 kind：wechat_keys、wechat_decrypt、image_key、export_all、export_history、export_voices、chat_plan、chat_plan_review、chat_plan_apply、decode_images、sns_decrypt。任务管理的 list/get/cancel/events 和受控 submit 是执行方式，不是所有业务的替代接口。MCP 任务默认关闭，宿主授权与模型请求不能互相替代。详见[任务服务](daemon-tasks.md)。

原始 SILK 与证据交付边界保持不变。语音识别、音频转码、模型管理与转录回写已经移除，不是待补缺口；语音目录也不承诺 SILK 能在浏览器原生播放。

## 源码依据与验收层次

| 层次 | 关键实现 | 证据边界 |
| --- | --- | --- |
| CLI | [命令树](../src/cli/mod.rs)、[查询细节](../src/cli/query_details.rs)、[映射测试](../src/cli/query_details_tests.rs) | 解析/映射测试不替代 CLI 进程业务测试。 |
| MCP | [schema 与路由](../src/mcp/protocol.rs)、[账号宿主](../src/daemon/mcp_service.rs)、[协议测试](../src/mcp/readonly_tests.rs)、[业务分派测试](../src/daemon/mcp_query_tests.rs) | 注册、参数测试和进程内真实源测试分别有用；不自动证明 stdio/命名管道链路通过。 |
| HTTP | [路由](../src/web/mod.rs)、[解析](../src/web/read_queries.rs)、[Call 校验](../src/service/web.rs)、[映射](../src/daemon/web_service/read_queries.rs) | 真实 HTTP 加模拟 Web 回复只证明传输与接线，不证明每项真实数据库行为。 |
| 查询业务 | [Request](../src/ipc.rs)、[查询分发](../src/daemon/server.rs)、[查询实现](../src/daemon/query.rs) | 需按同一账号、消息身份和业务状态跨入口核对。 |
| Web | [页面](../src/web/assets/index.html)、[事件与呈现](../src/web/assets/app.js) | 浏览器使用合成账号检查交互；不能由 DOM 接线推导业务结果正确。 |
| 任务子集 | [能力与计划](../src/service/plan.rs) | 只算已注册且授权的 kind/选项，不以内部 Step 数量或任意 Operation 替代公开能力。 |

检查要求见[测试计划](testing-plan.md)。是否运行和通过应由对应测试结果证明，不能从本文的“已接通”推导。
