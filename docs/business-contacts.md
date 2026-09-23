# 联系人与群成员

## 当前边界

- `business/contacts.rs` 定义账户范围内的 `ContactId`、`Contact`、`Member`、`Tag`、查询页、能力状态和窄只读 `ContactSource`。业务用例负责筛选、分页、显示名优先级、唯一身份选择及成员排序，不依赖 SQLite、微信、daemon、clap 或 JSON Value。
- `adapters/wechat/contacts/mod.rs` 实现接口，仅接受宿主传入的固定账户路径。实际探测联系人、标签和群成员所需表列，解释身份、认证与可见性，读取事务内将数据转换为业务对象。SQL 行号、成员关联编号和标签存储编号不进入业务模型。
- 适配器子模块保留已有群昵称 BLOB 算法和标签字段解析算法。严格标签与 raw 导出共用 `adapters/wechat/contacts/label_values.rs` 的标量及 field-30 解析，不依赖执行宿主。
- daemon 保留 `DbCache`、查询租约、名称缓存和响应投影。`load_names` 使用业务对象填充原有 `Names`；消息查询共享该缓存。

## 实际接线

CLI、MCP contacts 通过 `q_contacts` 消费当前查询租约的 `Names` 快照，投影 `{contacts:[{username,display}],total}`。HTTP `/api/contacts` 通过 `WebContacts`、`q_web_contacts` 从当前账号的 `SqliteContacts` 读取正式联系人业务对象；`contact_rows.rs` 的 Web 投影另含 `nickname`、`remark`，供界面优先显示微信昵称。members、contact tags 和 tag members 保留原有业务能力，`mcp_contacts.rs` 仍负责标签投影和固定账户装配，不包含 SQL 或 BLOB 解析。

正式联系人契约与保留的业务能力：

- 普通联系人只列真人，按显示名、身份排序；搜索显示名和身份。姓名相同时用身份稳定排序。
- CLI、MCP contacts 消费同一账号查询租约的 `Names` 物化快照，由微信缓存适配器转换为业务目录，不重新打开数据库或扩大快照范围。已物化的首选显示名不会被误标为昵称或备注；Web 的昵称和备注来自当前账号联系人数据库的独立字段。
- Contacts IPC 采用独立严格请求结构，旧 `legacy_view`（即使 false/null）和未知字段明确拒绝。MCP 只保留 JSON-text 协议包装，不新增联系人筛选或投影规则。
- 联系人读取、名称缓存、成员关联与唯一身份选择均拒绝重复身份。昵称、备注、别名、描述、电话及群身份仍由适配器保留供正式业务使用，不包含在 get_contacts 的两字段投影中。
- 标签定义的重复 ID 更新、重复关联计数及空标签名维持既有兼容规则。业务唯一标签选择采用不区分大小写的精确匹配优先，不允许入口自选首项。
- 群查询按稳定身份优先，其次唯一名称匹配；同名不能按扫描顺序任选一个。不存在、歧义、格式不支持、数据不可用、损坏数据和限额均为明确错误。

## 完整性与预算

正式联系人源为空时返回 `Unavailable`；有效源经过筛选无匹配则返回零项。这与连接级 display_names 空映射兼容语义不同。独立 contact-rows 夹具验证二者区别及失败读取不修改源文件，不使用成功空列表掩盖尚未就绪的账号源。

完整成员能力根据 `chat_room`、`chatroom_member` 和联系人关联列实测；群名列支持已存在的三个变体。完整成员为空时保留“完整但空”，不自动用历史消息填充。损坏关联、重复身份或重复群记录均失败，不吞掉错误。

确实缺少完整成员能力时，宿主才加载已知消息分库。结果增加 `membership_complete` 与 `membership_source`：`member_directory` 表示完整成员目录，`observed_senders` 仅表示记录中观察到的发言人。缺失分库、未知分库或无法解析的发言人关联不能伪装成成功空列表。此降级不证明成员目前仍在群内。

适配器保持只读事务与有限预算：联系人最多 100,000 条，单文本最多 4096 UTF-8 字节，累计联系人文本最多 16 MiB。标签保留原有 10,000 定义、100,000 关联、1 MiB 单 BLOB 和 16 MiB 累计文本上限。统一联系人 JSON 投影继续限制响应文本量。跨库读取不声称具有全局事务一致性。

## 原始导出与合成回归

### Raw 导出元数据边界

`adapters/wechat/contacts/raw_export.rs` 拥有联系人导出 SQL、只读快照和旧格式回退。`RawContactMetadata` 的动态 JSON 字段仅用于显式 raw 导出，不能作为普通 `Contact` 或 `Tag`。普通导出与 delta 的生产入口直接调用此适配器。

此 legacy profile 保留群聊不打开数据库、四个默认字段、字段与标签独立回退及 `metadata_warnings`。它保留 username 精确字节匹配、重复联系人首行、可选列精确大小写、`local_type != 3` 的 NULL 行为，以及数字原值、零/NULL/空 BLOB 转空字符串。标签保留重复定义覆盖但首次位置不变、重复关联计数、Unicode 数字和宽容 field-30 解码。严格标签仍要求字符串并执行既有预算，不能用 raw 回退代替严格错误；共享解析不意味着两个投影具有相同排序、值类型或失败策略。

普通 raw 导出可保留数字 JSON；delta 字符串 DTO 会拒绝数字元数据，不自动字符串化。适配器测试覆盖 raw 数字标签与严格类型拒绝，真实 delta 查询测试覆盖两个导出入口的这一差异。`contacts/raw_export/tests.rs` 包含 golden、只读和 schema/损坏库诊断测试。独立 fixture 通过真实联系人 adapter 加载测试。

业务内存测试覆盖筛选、分页、账户隔离、同名歧义和标签选择；合成 SQLite 测试覆盖能力缺失、列名变体、成员关联损坏、完整空群、历史发言人、重复身份、标签 BLOB、错误分类、读取预算及连接级显示名读取；名称缓存适配测试验证快照范围、首选显示名与分类。联系人 fixture 覆盖正式两字段输出、筛选、排序、total、零 limit 和只读安全检查；根 MCP 测试覆盖合成双账号正式查询一致性及不支持参数的拒绝。已有标签与群昵称解析测试继续复用生产实现，空名称缓存仍必须报错。执行方式见[测试说明](../tests/README.md)。

SNS 导出通过适配器的连接级 `display_names` 读取联系人，不自行查询联系人列；目录名清理留在导出边界。该接口保留空表为空映射，并可使用调用方已有事务。独立 `contact-rows` 夹具直接导入真实联系人业务、适配器和元数据解析模块，不引入其他业务域或替代实现。
