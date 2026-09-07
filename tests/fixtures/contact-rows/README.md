# 联系人返回行契约

初次交付仅新增 `src/daemon/query/contact_rows.rs`，当时未注册公共 root。

> 2026-09-07 文档核对：当前 `query.rs` 已注册 `contact_rows`；`mcp_contacts_legacy.rs` 和 daemon 的 Contacts `legacy_view` 分支负责旧返回行视图，默认 CLI 联系人视图独立保留。下方接线建议和独立测试数字是历史证据，不是当前待接线清单，也不是本次新增通过结果。

## 旧代码证据

- `vendor/wechat-decrypt/mcp_server.py:250` 的 `_load_contacts_from`：存在精确名称 `local_type` 列时 SQL `!= 3`，否则不筛选。保留群、公众号、系统入口及重复 username；NULL local_type 被 SQL 排除。
- 旧六字段：username、nick_name、remark、alias、description、phone。电话依次取 phone、phone_number、mobile、mobile_phone、telephone 的首个非空值。可选列名称精确匹配。
- 同文件 `get_contacts`：默认 limit=50，仅昵称、备注、username 的 lowercase 子串搜索；不 trim、不按 alias/电话/描述搜索。先算 total 再截取，保持 SQLite 返回顺序。
- 现有 `query.rs::q_contacts` 仅 private 类型，使用已折叠的 Names.map，按 display 排序；会丢失被备注遮盖的昵称及重复 username。

## 历史最小接线建议

1. 主线程注册 `mod contact_rows;`。
2. 为 `q_contacts` 传入 `&DbCache`，调用方从本账号 `contact/contact.db`（必要时同账号反斜线键）获取真实路径；禁止跨账号或全局副本回退。
3. 将路径、owned query、limit 移入 `tokio::task::spawn_blocking`，调用 `contact_rows::contacts_from_path(&path, query.as_deref(), limit)`。
4. server 的 Contacts 分支传入 DbCache；IPC 默认 limit=50 和 MCP 参数约束无需改变。错误仍由既有 MCP 安全错误映射隐藏。

返回 JSON 保留 contacts、username、display、total；每行增加旧字段。display 依次取备注、昵称、username。若现 CLI 必须继续 private-only 且按 display 排序，应在主线程明确保留独立视图；本 helper 不能同时把旧 MCP 全范围与该旧 CLI 过滤行为当作同一默认行为。

没有新增 protobuf 解析。既有 contact_metadata 的单联系人转换入口私有，导出入口会逐人重扫并读取标签；此 helper 使用 SQLite ValueRef 的最小文本转换，不调用该导出接口。若后续需要合并标量转换，只需将既有转换器提取为共享文本 API，不应复制标签解析器。

## 验证和边界

独立 Cargo fixture 读取真实临时 SQLite，oracle.py 只提取旧两个函数 AST，不导入 MCP/配置；20 组现代/旧 schema 与查询组合比较旧返回行和 total。另覆盖可选 NULL、电话优先级、字面量 SQL 注入字符串、缺失库不创建、空库/畸形字段/超限拒绝，数据库字节保持不变。

limit=0 保留空结果及 total；usize 不支持旧 Python 的负数切片。空库或无合格行报错，非空库无匹配返回空 contacts。保留旧数据库顺序而非原 CLI display 排序。NULL/零/空 blob 转为空串；非零数值、非空 blob、非法 UTF-8 及空 username 明确拒绝，不伪造联系人字段。

限制为查询/单字段 4096 UTF-8 字节、合格扫描行 100000、返回行 JSON 累计 16MiB；超限报错，不静默截断 total。不是数据库路径沙箱，来源与账号绑定由调用方保证。

执行：`cargo test --offline --manifest-path tests/fixtures/contact-rows/Cargo.toml --target-dir C:/CodexLocal/build/contact-rows`。Python 仅供测试 oracle；生产无 Python 依赖。
2026-09-07 实跑：3 passed、0 failed、0 ignored，无编译警告。日志：`C:/CodexLocal/日志/contact-rows-tests.log`。不代表已接公共 IPC 或完成全仓回归。
