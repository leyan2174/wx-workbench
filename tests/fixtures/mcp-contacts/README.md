# MCP 联系人标签查询

生产实现：`src/daemon/query/mcp_contacts.rs`。初次独立交付没有修改公共注册、IPC、MCP 协议或 ASR；当前模块、daemon 的 ContactTags/TagMembers 分支及 MCP 工具均已接线。

> 2026-09-07 文档核对：本目录的独立 Cargo harness 早已撤除，本次清理的是遗留 `target/` 生成物，不是 `tests.rs`、oracle 或历史日志。下面的主仓命令是现行入口；本次只更新文档，没有新增测试运行结果。

## 接线接口

- `q_contact_tags(&DbCache, &HashMap<String, String>) -> Result<ContactTags>`
- `q_tag_members(&DbCache, &HashMap<String, String>, &str) -> Result<ContactTag>`
- 名称映射必须来自同一账号。函数仅查传入缓存的 contact/contact.db 或反斜杠历史键。
- 生产接线见 `src/daemon/query.rs`、`src/daemon/server.rs` 和 `src/mcp/protocol.rs`。本目录的查询测试仍不等同于完整 MCP 进程验收。

## 语义

定义按 sort_order_ 排序，重复 ID 后值覆盖且保留首次插入顺序。关联来自 extra_buffer 第一个 protobuf field 30；未知字段跳过，重复 ID 和重复联系人行均重复计数，空名称标签保留。支持旧 Python Unicode 十进制 ID、符号及合法下划线。匹配先忽略大小写精确匹配，再忽略大小写包含匹配；任何多个匹配均报歧义，包括重复精确名称。空查询可以精确选择唯一空名称。

旧实现会把数据库故障吞成空标签；本实现明确报错。异常 NULL/非文本名称、非整数排序值等不兼容 schema 明确失败，不伪造结果。数值 ID 保留 Python 数值相等语义，文本 ID 不与数字混同。protobuf 保留旧切片截断行为，并有机器整数溢出保护。

直接复用 `crate::toolkit::contact_metadata::{parse_label_id, sqlite_id_equal, extract_field_30}`，分别命名为 label_id、id_equal、field_30。本文件不再保留重复解析器，也不修改共享实现本体。

## 资源上限

- 最多 10,000 条标签定义，包括重复 ID 行；在 ORDER BY 和字段复制前用有界计数检查。
- 最多 100,000 个关联，跨联系人行累计，重复关联计入限额；检查先于成员字符串复制和追加。
- 单个 extra_buffer 最多 1 MiB，以 ValueRef 借用检查，不能先复制为 Vec；即使首个字段很小，也检查整个 buffer。
- 查询名称及每个文本字段最多 4 KiB UTF-8 字节。查询名称在 trim、大小写转换和 DbCache 访问之前验证；保留空名称精确匹配。
- 累计复制的标签名称与成员文本最多 16 MiB，重复覆盖的名称也计入预算。此预算不等同于 JSON 序列化长度；MCP 输出上限仍由协议层执行。
- 上限值本身允许，多 1 拒绝；任何超限整次失败，无部分结果，错误不回显 username 或标签内容。

## 验证边界

生产模块通过 cfg(test) 引用本目录 tests.rs，全部测试直接由主仓编译。已移除独立 Cargo harness 与缓存替身；使用真实 DbCache::with_dirs 注入合成缓存命中，同时核验反斜杠键、账号边界和坏密钥前的查询校验。此处不覆盖完整加解密或主仓 IPC 接线。

oracle 只从旧源码 AST 提取两个标签 helper，不启动旧服务、不访问真实账号或网络。设置 CONTACTS_ORACLE_PYTHON 可选择本机 Python；生产代码无需 Python、新 Rust 依赖或配置读取。主仓运行 `cargo test --bin wx daemon::query::mcp_contacts::tests`，保留旧差分测试与各限额边界测试。
