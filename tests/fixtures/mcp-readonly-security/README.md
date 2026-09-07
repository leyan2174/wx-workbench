# 只读标签与引用模块独立审查

> 本页保留修复前后各次审查证据，测试数字不代表本次清理后的新运行。当前生产已接线，但本独立 harness 的替身边界仍成立。新复跑应另存日志，不覆盖原红测、修复输出或源指纹；本次没有执行 Cargo。

## 历史修复结果

2026-09-07，Windows x64 MSVC。初次审查只新增本目录；随后获明确授权，修复 mcp_refer.rs、其 query_tests.rs 和本 fixture。没有修改 meta.rs、query.rs 共享根文件、mcp_runtime 或 ASR。

修复后主仓引用测试 15 passed、独立安全测试 15 passed，均 0 failed、0 ignored。**原两项漏洞复现已改为明确拒绝并检查源字节不变；旧 latest-tests.log 的 13 项绿色仅是修复前审查记录，不是修复证明。** 当前结果见 fixed-main-tests.log 和 fixed-security-tests.log。主仓 cargo check 通过；日志中的并行 ASR cache 未使用项警告不属于本次改动，未越权处理。

主仓 query 已注册本模块，主仓测试使用真实 DbCache 和合成缓存命中；本报告不声称验证了最终 MCP/IPC 接线、真实缓存解密或真实账号。

## R1 / P2：未知大写消息分片（已修复，保留威胁模型）

修复前位置：mcp_refer 的前后清单检查，依赖旧 advisory 枚举判断唯一性。

合成复现：仅将 message/message_0.db 放入已知清单，另建 MESSAGE_1.DB；两片 Alice 的目标表都包含 local_id=7/create_time=100 的引用回复。调用真实 q_decode_refer 和真实 discover_unknown_shards，返回 `exit_code=0`、source=message/message_0.db。原文件字节未变。把未知片原样改名为 message_2.db，同一查询就返回未知分片错误。

当前回归：`uppercase_unknown_shard_rejects_duplicate_identity_without_mutation`。大写和小写未知分片均被真实 checked helper 拒绝，查询前后数据库字节不变。

威胁模型：Windows 快照、恢复或外部创建的分片名可改变大小写；并不声称微信日常创建的文件名一定采用大写。旧 meta helper 本来用于新鲜度提示；风险是新模块把这个不完整枚举当作严格唯一性的安全依据，不能据此交付“唯一消息”的成功结果。

落实：主线程提供 discover_unknown_shards_checked/ensure_complete_message_inventory，引用查询前后直接调用后者。独立 fixture 委托真实 checked helper，不调用吞错的 advisory wrapper。补充 known 大写反斜杠 raw 键正常命中、noDir 返回错误、目录伪装成分片拒绝、原有读取后新增片拒绝测试。

## R2 / P2：同名视图误当缺表（已修复，保留威胁模型）

修复前位置：mcp_refer::lookup 的 sqlite_master type='table' existence 查询。

合成复现：两个已知分片均有同一 local_id/create_time，正常查询返回歧义 `exit_code=2`。随后在第二片将目标 Msg_<md5> 表改名为 PreservedRows，再创建原目标名称的 `SELECT * FROM PreservedRows` 视图，保留全部消息行。查询现在返回第一片的 `exit_code=0`，原因是 catalog 查询只找 type='table'，把仍然存在但不受支持的同名对象当作“此片无该表”跳过。

当前回归：`target_named_view_is_rejected_without_mutation`，要求错误链包含 unsupported message table schema，查询前后所有分片文件字节一致。

威胁模型：不可信或发生 schema 漂移的已解密 SQLite；不声称普通旧微信库有此视图，也不要求支持视图查询。这里应当拒绝不受支持的模式，而不是读取视图，更不是跳过后报唯一成功。

落实：lookup 使用 pragma_table_list，只有确实不存在的对象可以跳片；必须是 type='table' 且 wr=0 的普通 rowid 表。同名视图、virtual/shadow 或 WITHOUT ROWID 明确 Err。主仓新增正常缺表可跳片、普通表大小写变体可读测试。主仓 SQLite 只有 json_tree/json_each 模块，因此虚拟表测试通过合成外部 SQLite 构建的虚拟表 catalog 验证实际 schema 拒绝，不启用扩展、不忽略测试；该用例实际返回预期错误且文件不变。

## 已通过的检查

- 引用联系人同名、大小写不同的精确同名及模糊多匹配均 `exit_code=2`，无 refer 内容；缓存调用次数和真实枚举调用次数都是 0。精确 username 仍可访问正确对象。
- 跨片 local_id 在类型过滤前验证唯一性；同时间跨片重复仍拒绝。create_time=0 保持旧契约的“不筛选”，不是时间戳零的专门筛选。
- 已知片丢失、坏 SQLite、目标表缺必需列、新增小写未知片均返回 Err，不把第一片命中当部分成功。缓存解析期间注入新片，第二次真实清单核验会拒绝。
- 有效 zstd BLOB 和 flag=4 的 TEXT 按各自契约处理；损坏压缩、超出 128 KiB 的解压数据、DTD/实体、坏 XML、超过共享 20,000 字符上限的 XML 都拒绝。存储正文超过 1 MiB 返回明确错误。
- 合成 XML 内 AES/CDN 标记不会在图像引用摘要或错误路径泄露；错误不回显原 XML/坏压缩内容。未声称用户本来允许读取的普通文本内容会被自动“去秘密”。
- 标签 SQL 数值 1 与 1.0 共享数值身份，文本 '1' 不混为数字；超过 2^53 的相邻 integer/real 和 i64::MAX/2^63 实数边界不误关联。
- 重复标签 ID 按既定覆盖顺序处理；重复 field-30 ID、重复联系人行保留关联数。显示名相同但 username 不同，不被合并。重复计数是明确契约，不是漏洞。
- 标签精确/模糊多匹配都报歧义；唯一空名称可由空查询选择，这是声明的旧语义。
- 后续坏关联行整次报错，不返回此前已收集成员，错误不回显合成秘密值。
- owner 最新实现的 1 MiB buffer 上限两端验证：恰好上限通过，上限+1 字节及 2 MiB 被拒绝。
- 约 20 KiB 的重复关联字段，若展开将超过 16 MiB 成员字符串，则返回 result text byte limit 错误；不会继续无限复制。重复计数语义仍保留。
- 超过 10,000 条标签定义（包括重复 ID）、100,000 条关联均明确拒绝。4,097 字节原始查询在任何缓存访问前拒绝，不能用 trim 绕过。
- 主要成功、拒绝与风险复现用例均核对合成源文件字节不变。不创建真实密钥，不访问真实账号或网络。

## Harness 边界

`lib.rs` 引用真实 mcp_contacts.rs、mcp_refer.rs、message/mod.rs、daemon/meta.rs 和 contact_metadata.rs；不重写标签、消息查询、解压、XML、摘要或磁盘枚举算法。

DbCache/Names 为接口形状替身：缓存记录调用并返回合成路径，保留源文件消失返回 None 的边界；可在 get 时复制一份合成新片，用于验证前后清单检查。ensure_complete_message_inventory 仅加计数，再调用真实 discover_unknown_shards_checked 并检查 unknown 为空，与 query.rs 严格适配器同形。私有 msg_table_re 使用与 query.rs 相同的固定 Msg 正则。独立 fixture 本身不验证真实加密缓存或最终公共路由；另有本次主仓 query_tests 的真实缓存命中及读取后时点测试。

使用独立 lib + integration test，禁用 lib 自身 test/doc-test，避免意外编译需要完整真实缓存的其他 owner fixture。只执行本目录 security.rs 的 15 项测试。

## 命令与日志

现行独立审查入口（仓库根目录，另存新输出）：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-readonly-security/Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture
```

以下是当时的命令记录，包含格式化及固定日志名，不应直接重跑覆盖历史证据：

```powershell
rustfmt --edition 2021 --config skip_children=true tests\fixtures\mcp-readonly-security\lib.rs tests\fixtures\mcp-readonly-security\security.rs
cargo check --offline --manifest-path tests\fixtures\mcp-readonly-security\Cargo.toml --target x86_64-pc-windows-msvc 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\latest-check.log
cargo test --offline --manifest-path tests\fixtures\mcp-readonly-security\Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\latest-tests.log
Get-FileHash src\daemon\query\mcp_contacts.rs,src\daemon\query\mcp_refer.rs,src\daemon\meta.rs,src\toolkit\contact_metadata.rs,src\message\xml.rs,src\message\export_content.rs -Algorithm SHA256 | Format-List | Out-String -Width 4096 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\source-hashes.log
```

latest-check.log/latest-tests.log 保留完整标准输出与错误输出；source-hashes.log 为审查时的真实源指纹。owner 仍可能修改源码，复跑应以新指纹和新结果为准。

修复后的结果另存，避免覆盖修复前威胁证据：

```powershell
$env:LIBCLANG_PATH = 'C:\CodexLocal\build-tools\libclang\clang\native'
cargo check --target x86_64-pc-windows-msvc 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\fixed-main-check.log
cargo test --target x86_64-pc-windows-msvc --bin wx mcp_refer -- --nocapture 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\fixed-main-tests.log
cargo test --offline --manifest-path tests\fixtures\mcp-readonly-security\Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\fixed-security-tests.log
Get-FileHash src\daemon\query\mcp_refer.rs,src\daemon\query.rs,src\daemon\meta.rs -Algorithm SHA256 | Format-List | Out-String -Width 4096 | Tee-Object -FilePath tests\fixtures\mcp-readonly-security\fixed-source-hashes.log
```

最初 check.log/tests.log 的编译错误是 owner 同时引入真实 contact_metadata 依赖后，本 harness 尚未更新模块路径造成的，已同步解决；不是生产缺陷。原资源预算候选在首次成功实跑前已由 owner 补齐，现已改为明确拒绝断言，不作为漏洞报告。
