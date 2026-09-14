# 联系人返回行契约

`contact_rows` 由 daemon 查询层注册，Contacts 的 `legacy_view` 使用原始联系人行；默认 CLI 联系人视图独立保留。

## 返回语义

- `vendor/wechat-decrypt/mcp_server.py:250` 的 `_load_contacts_from`：存在精确名称 `local_type` 列时 SQL `!= 3`，否则不筛选。保留群、公众号、系统入口及重复 username；NULL local_type 被 SQL 排除。
- 旧六字段：username、nick_name、remark、alias、description、phone。电话依次取 phone、phone_number、mobile、mobile_phone、telephone 的首个非空值。可选列名称精确匹配。
- 同文件 `get_contacts`：默认 limit=50，仅昵称、备注、username 的 lowercase 子串搜索；不 trim、不按 alias/电话/描述搜索。先算 total 再截取，保持 SQLite 返回顺序。
- 默认 CLI 联系人视图使用 Names 映射及 display 排序，不能替代 MCP 的原始行视图。

## 调用方

路径由固定账号的 contact/contact.db 提供，不回退到跨账号或全局副本。`contacts_from_path` 接收路径、query 和 limit；账号绑定由上层负责。

返回 contacts 和 total，每行包含 username、display 及联系人字段。display 依次取备注、昵称、username；不能把 MCP 全范围视图与 CLI 的筛选排序当作同一默认行为。

## 测试与边界

独立 Cargo fixture 读取真实临时 SQLite，oracle.py 只提取旧两个函数 AST，不导入 MCP/配置；20 组现代/旧 schema 与查询组合比较旧返回行和 total。另覆盖可选 NULL、电话优先级、字面量 SQL 注入字符串、缺失库不创建、空库/畸形字段/超限拒绝，数据库字节保持不变。

limit=0 保留空结果及 total；usize 不支持旧 Python 的负数切片。空库或无合格行报错，非空库无匹配返回空 contacts。保留旧数据库顺序而非原 CLI display 排序。NULL/零/空 blob 转为空串；非零数值、非空 blob、非法 UTF-8 及空 username 明确拒绝，不伪造联系人字段。

限制为查询/单字段 4096 UTF-8 字节、合格扫描行 100000、返回行 JSON 累计 16MiB；超限报错，不静默截断 total。不是数据库路径沙箱，来源与账号绑定由调用方保证。

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/contact-rows/Cargo.toml
```

Python 只用于 oracle，生产不依赖它。
