# 原生附件查询边界审查

> 历史修复前审查：本页的五项失败及预期退出码原样保留，不是当前失败状态。同目录 [SCALAR-FIX.md](SCALAR-FIX.md) 记录了保持原断言后的修复复验。本次只同步文档，未运行测试，也不把后续通过数字填回本页。

## [P2] 结构化约束被降级为空，查询可成功返回错误附件

位置：`src/toolkit/attachment_refs.rs:139` 的 text、142 的 descendant_text，以及 278/329 的 MD5 取值。helper 使用 Node::text()，没有拒绝字段内的子元素或混合内容。因此 `<md5><value>900150983cd24fb0d6963f7d28e17f72</value></md5>` 变成缺少 MD5；嵌套 totallen 同样变成大小未知。混合内容只保留第一个合法文本片段。

实际 SQL 复现：单个 Msg 表中放入唯一 local_id=7、create_time=100、local_type=49 的文件消息，文件名 report.txt、大小 3、上述嵌套 MD5；账号缓存目录里放三个字节 `bad`。真实 strict_message 定位后，真实 q_attachment_reference 返回 `exit_code=0`、`status=found`、`expected_md5=null`，reference 为 `binding=heuristic`、实际 MD5 `bae60998ffe4923b131e3d6e4c19993e`。对应测试 sql_nested_hash_must_not_publish_wrong_heuristic_reference；完整响应保存在 query-boundary-audit.log。

同一字段若为普通文本 MD5，已有纯 API 对照用例返回 HashMismatch；记录项 fullmd5 也有相同降级。五条失败是同一个解析完整性问题，不计作五个漏洞。输出确实带有 heuristic 警告，因此不声称伪造了 MD5 强绑定；问题在于已经提供但结构错误的约束被当成未提供，查询仍成功返回错误文件。修复建议：标量元数据存在时拒绝子元素/混合内容，仅真正缺失或允许的空值才能降级。生产未由本审查修改。

## 验证与范围

初次检查发现本目录已经存在 refs_security.rs、image_security.rs 和 Cargo 接线，未覆盖重建。保留原断言，仅修既有 image junction fixture 的 `System32/cmd.exe` 混合分隔符拼接；原失败发生在命令创建 junction 阶段，修路径后测试通过，不列为生产漏洞。

本轮只新增五项 query_boundary SQL 测试与最小模块接线，复用既有 mcp-readonly-security 的合成 DbCache/Names 容器。SQLite、strict_message、mcp_attachments、attachment_refs、meta::discover_unknown_shards_checked 全部为真实源；query.rs 私有正则和清单 wrapper 在 fixture 中同形接线。没有复制查询实现、真实账号发现、模型、网络或 daemon 启动。

最终 21 项：16 通过、5 上述解析问题失败、0 忽略。没有为凑覆盖扩展重复的纯 API 测试。

- 两个分片重复身份先返回歧义，不按 local_type 或记录 item_index=0/999 选择其中一条。
- 已知缺片、缓存未加载、坏 SQLite、未知分片、第一次 get 期间新出现分片均返回完整性错误，不被记录未加载或索引越界吞掉。
- 查询缓存根来自当前 DbCache.db_dir 的父目录；同消息身份的两个合成账号不会交叉搜索。匹配账号成功，另一账号文件 MD5 错误时失败，不回退查找。
- 普通文件 20,000、记录 500,000 解压字节上限及 1 MiB 存储字节上限真实失败；不声称 SQLite 内部或进程峰值内存限制。
- 既有纯 API 测试验证账号根分离、跨记录候选歧义、路径遍历/ADS 拒绝、只读句柄保护硬链接别名。既有 image SQL 测试的目录 junction、输出与资源库硬链接、同身份资源歧义、元数据字节限额均通过。
- 查询前后比较临时数据库/文件的完整字节与目录清单；故意引入新片的测试只允许该测试注入的新文件，原有文件字节均保持不变。诊断不含测试 CDN/key 标记。

合成缓存容器不证明真实 daemon 的配置加载、解密或账号认证；手动根目录纯 API 本身也不认证账号。未操作 MAIN 全量测试 session25438。证据版本见 source-hashes.log。

## 现行复跑入口

在仓库根目录执行；本次文档核对没有执行此命令：

```powershell
cargo test --offline --manifest-path tests/fixtures/native-attachment-security/Cargo.toml --target-dir C:/CodexLocal/build/native-attachment-security -- --nocapture
```

另存本次输出，不覆盖 `query-boundary-audit.log` 或修复日志。

## 历史复跑命令

以下命令是产生修复前证据时的记录，不应直接复跑覆盖历史日志：

```powershell
cargo test --offline --manifest-path tests/fixtures/native-attachment-security/Cargo.toml --target-dir C:\CodexLocal\build\native-attachment-security -- --nocapture 2>&1 | Tee-Object -FilePath tests/fixtures/native-attachment-security/query-boundary-audit.log
exit $LASTEXITCODE
```

当时预期退出码 101；保留红测试等待生产修复，不以过滤失败项或放宽断言取得通过。
