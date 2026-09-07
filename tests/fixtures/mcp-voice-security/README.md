# MCP 语音独立审查

> 本页数字属于修复前后各次历史运行，本次文档同步没有重新验证或替换它们。源码与测试仍保留；下面固定文件名的命令是历史证据生成记录，不应直接重跑覆盖旧日志。

## 结论

2026-09-07，Windows x64 MSVC。修复后主仓 `cargo check` 零警告，主仓 ASR 84 passed；联合独立测试 45 passed、0 failed、0 ignored，doc-tests 0。此次结果包含 9 项独立安全测试和 36 项真实模块测试。

F1 现断言返回错误，完整错误链必须含 `unsupported rowid schema`，并检查数据库文件字节不变。F2 现断言大写和小写重复分片均返回 `AmbiguousMedia`，第一次拒绝后检查源文件字节不变。**旧 tests.log 的 34 passed 仅为修复前漏洞复现，不是修复证明；当前修复证据以 fixed-tests.log 中的 REGRESSION F1/F2 和新增主仓拒绝断言为准。**

本目录允许 path 引用真实源代码，未复制生产查询逻辑。DbCache 只是公开接口形状替身；使用最新 `q_voice_messages(db, &query)`，由 `media_db_keys()` 枚举 raw 键。不访问真实账号、密钥、解密缓存或音频，不做真实解密集成声明。初次审查只新增本目录；后经用户明确授权，修复 database_media.rs 并新增其主仓测试。F1 生产修复由 Laplace 完成，未改其模块。ASR loopback 测试不再扩改。

## 原始威胁模型与修复

### F1 / P2：遮蔽 Name2Id.rowid 可造成跨联系人错误归属

位置：`src/daemon/query/mcp_voice.rs` 的 Name2Id/VoiceInfo 行标识查询，以下为修复前威胁模型。

复现：建立旧字段结构的 Name2Id，SQLite 内部行号 1 属于 Alice，2 属于 Bob；为该表增加普通 `rowid` 列，将 Alice 的列值设为 2。查询 Alice 时，生产列表函数接受该模式并返回 Bob 的 local_id=802，输出 username 仍为 alice。同一库交给 database_media，会返回 UnsupportedSchema。

这是模式漂移或不可信 SQLite 输入下的归属验证缺口，不声称正常旧数据库普遍包含该列。重复 username 检查不能拦截，因为 Alice 仍只有一行。仅改用 `_rowid_` 也不完整，该名称同样可被用户列遮蔽。

已验证修复：Laplace 在列表入口要求普通 rowid 表，并通过 table_xinfo 拒绝 `rowid`、`_rowid_`、`oid` 同名列（含生成列）。独立测试现为 `security_shadowed_name_rowid_is_rejected_before_cross_contact_attribution`，要求指定模式错误而非任意成功/空列表，且源字节不变。

### F2 / P2：大写媒体分片名被 ASR 静默跳过，唯一性判断失效

位置：`src/toolkit/asr/database_media.rs::media_shards`，以下为修复前威胁模型。

复现：message/message_0.db 的 Alice 语音 server_id=900；media_0.db 和 MEDIA_1.DB 均有 Alice 对应的同 server/time 记录。MCP 显式双分片列表返回两条；ASR 却成功选择 media_0.db。仅将 MEDIA_1.DB 改名为 media_2.db，完全相同的数据库内容立即导致 AmbiguousMedia。

Windows 文件系统通常大小写不敏感，但 Rust 的 starts_with/ends_with 是大小写敏感的。快照并未缺片，枚举器自行漏掉文件，因此不同于“调用方须提供完整快照”的文档边界。

已修复：候选识别使用 ASCII 小写，仍严格要求 media_数字.db；保留实际文件名打开和复核快照，证据 source 规范为小写，数字部分不改写。规范 source 重名或指向同一物理文件的硬链接别名均拒绝。新增主仓测试覆盖大写单片、重复匹配、非法大写候选和无目标联系人的硬链接别名；独立 `security_uppercase_media_shard_cannot_hide_asr_ambiguity_on_windows` 要求 AmbiguousMedia。

### 同轮补充：ASR 完整列与表类型校验

database_media::require_table 改为检查 pragma_table_list 的普通 rowid 表，并通过绑定参数的 pragma_table_xinfo 读取完整列。新增主仓测试验证三张证据表的生成 rowid/_rowid_/oid 列均拒绝（虚拟生成列全链路、存储生成列直接模式校验）；视图和 WITHOUT ROWID 明确返回 UnsupportedSchema。普通表和非保留名称的 INTEGER PRIMARY KEY 保持可用。没有引入跨模块抽象。

## 通过的检查

- 旧源证据为 vendor/wechat-decrypt/mcp_server.py 的 `_get_chat_name_id` / `get_voice_messages`：本库 Name2Id.rowid -> 本库 VoiceInfo.chat_name_id；列表读取 local_id/create_time/length(voice_data)，每片 limit+offset 候选后全局时间倒序。
- 两媒体库 Alice/Bob 编号交叉交换，仍只返回 Alice。COLLATE NOCASE 的表也不把 ALICE 当 alice。ASR 正确关联 message.server_id 与 media.svr_id，不把媒体 local_id 当消息 local_id。
- 720 种分页/时间范围/分片输入顺序组合对照独立内存排序结果；含相同时间、相同 local_id、多 rowid、media_2/media_10 字典序、越过页尾和 limit 大于全集。顺序为时间降序、source 升序、local_id 降序、rowid 降序，没有发现固定数据集下漏行或重排问题。
- since/until 两端包含；0 与负时间不按真假误处理。SQL NULL 长度保持 None，空 BLOB 为 Some(0)，带 NUL 的 BLOB 按字节数计算。
- 仅含旧列表字段而没有 svr_id 的模式可列出，ASR 明确拒绝 UnsupportedSchema。这是已有文档声明的关联证据要求，不作为兼容性缺陷报告。此次未取得真实用户数据库 DDL，不宣称覆盖全部微信版本。
- 最新适配器保留 `MESSAGE\\MEDIA_0.DB` raw 键传给 get，输出规范 source；规范化后重复键被拒绝。
- 账号已知分片缺失、未知额外分片、get 返回 None、损坏媒体分片均整次失败，不返回已有片的部分成功结果。
- 离线 query_voice_shards 只收到一个分片时会返回该片；ASR 完整快照中的第二片被调用方移除后也无法再检测其重复记录。这两个入口已声明依赖调用方提供完整快照，不误报为独立漏洞。拥有完整 keys 的账号适配器能拒绝同一缺片场景。

## 执行与日志

现行独立回归可从仓库根目录运行，另存本次输出：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-voice-security/Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture
```

以下保留历史执行命令；包含格式化和固定日志输出，不是本次文档同步执行过的操作：

```powershell
rustfmt --edition 2021 --config skip_children=true tests\fixtures\mcp-voice-security\lib.rs tests\fixtures\mcp-voice-security\security_tests.rs
cargo check --offline --manifest-path tests\fixtures\mcp-voice-security\Cargo.toml --target x86_64-pc-windows-msvc 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-voice-security\check.log
cargo test --offline --manifest-path tests\fixtures\mcp-voice-security\Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-voice-security\tests.log
Get-FileHash src\daemon\query\mcp_voice.rs,src\toolkit\asr\database_media.rs -Algorithm SHA256 | Format-List | Out-String -Width 4096 | Tee-Object -FilePath tests\fixtures\mcp-voice-security\source-hashes.log
```

check.log/tests.log/source-hashes.log 保留修复前审查证据。修复后命令及输出另存，避免把旧的漏洞复现绿色当作修复：

```powershell
cargo test --offline --manifest-path tests\fixtures\mcp-voice-security\Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture 2>&1 | Tee-Object -FilePath tests\fixtures\mcp-voice-security\fixed-tests.log
Get-FileHash src\daemon\query\mcp_voice.rs,src\toolkit\asr\database_media.rs -Algorithm SHA256 | Format-List | Out-String -Width 4096 | Tee-Object -FilePath tests\fixtures\mcp-voice-security\fixed-source-hashes.log
```

主仓修复后 check/ASR 全组日志：`C:\CodexLocal\日志\asr-media-final-check.log`、`C:\CodexLocal\日志\asr-media-final-tests.log`。并行工作可能继续改变源码，应以复跑结果为准。
