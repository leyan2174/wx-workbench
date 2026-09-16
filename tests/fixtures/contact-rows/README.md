# 正式联系人返回契约

此独立 fixture 直接导入生产联系人业务、微信适配器和 `contact_rows` 投影，不再调用 Python 或 vendor 旧工具 oracle。

## 唯一契约

`get_contacts`、CLI contacts 和 HTTP contacts 的后台查询均使用 `q_contacts`。MCP 仅将正式查询结果包装为 JSON-text：

```json
{"contacts":[{"username":"a","display":"Alpha"}],"total":1}
```

- 只列真人，按首选显示名、username 稳定排序。
- query 对 username 和首选显示名进行不区分大小写的子串匹配；不独立搜索隐藏昵称、别名、描述或电话。
- total 是截取前匹配数，limit=0 返回空数组并保留 total。
- Contacts IPC 和 MCP 参数仅允许 query、limit；旧 legacy_view 字段任何值均拒绝，不静默忽略。
- 正式 daemon 从当前账号查询租约的 Names 快照查询；空名称缓存仍由 daemon 拒绝，不伪装成成功空结果。

## 测试边界

fixture 的临时 SQLite 测试覆盖正式筛选、排序、数量、零 limit、字面量 SQL 字符串、缺失文件不创建、非法字段、超长输入、重复身份拒绝及只读字节不变。
完整联系人元数据和群身份仍由业务对象保留，但不进入简洁的联系人返回行。
适配器自身的群、标签、schema 能力和损坏数据测试仍随真实模块编译。
真实 SQLCipher/daemon/MCP 的双账号和协议拒绝覆盖位于根测试及 `query/contacts_source_tests.rs`。

测试运行由父任务协调。本次修改未运行 Cargo，不声明上述测试已经通过。
