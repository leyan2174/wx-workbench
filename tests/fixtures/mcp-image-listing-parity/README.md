# 图片列表字段缺口核验

2026-09-07：生产图片列表元数据已实现。最初的 AST golden/设计调查保留在下文；新增 `metadata_tests.rs` 直接测试生产 helper，`query_tests.rs` 直接测试生产 query 与真实 DbCache。协议投影由主线维护，本目录不修改协议。

> 本页测试数字对应下列各次历史日志，本次文档同步未重新执行 Cargo 或 oracle。“实证差异”是最初调查时的源码快照，其中旧字段缺口不应被当作上方实现完成后的当前状态。复跑另存日志，保留原 golden 和失败证据。

## 当前实现与验收

- `q_attachments` 保留原签名、轻量默认行为和返回字段；新增同参数 `q_attachments_with_image_metadata`，二者复用单一消息筛选、全局排序和分页实现。
- 分页前保留 raw local_type 和分片来源。分页后对页面身份在**全部已解析分片**执行绑定参数的精确计数，每个身份每片最多匹配两行；同片重复和其他片分页截断之外的重复均为 `message_ambiguous`，不会根据有大量同时间戳就把唯一身份误报为歧义。
- `native_image::ResourceReader` 共享导出原有的精确关联 SQL/校验，一个页面一个只读资源事务。标准文件名扫描共享 `scan_candidates`，一页一次有界目录扫描；列表对所有月份、所有变体的多候选统一返回 size 歧义，导出的原有 full/h/t 优先规则未改变。
- `attachment::image_metadata::read_page` 不调用密钥提供器、decoder 或发布函数，不读取 DAT 正文。size 来自 `Pin` 固定文件句柄的 metadata 长度；原始加密 DAT 可以是零字节或超过解码器的大小限制。
- 行 schema：`md5: string|null`、`size: u64|null`、`resource_status`、`size_status`、`size_kind="encrypted_dat_metadata"`、`binding="exact_resource_standard_filename_metadata"`。严格满足 found iff md5Some、available iff sizeSome、无 md5 不允许 sizeSome；缺失 MD5、歧义消息和未请求大小时 size_status 为 not_requested。不返回路径、packed_info 或密钥。
- 坏资源 schema、侧车文件和读取错误返回失败，不冒充 missing。每页最多 1000 行；目录、候选和资源大小沿用共享防护的有界限制。唯一性检查针对已解析分片，不声明覆盖未知分片或抵御所有同权限并发篡改。

验证日志（2026-09-07）：

| 命令 | 结果 | 完整日志 |
| --- | --- | --- |
| `cargo test --manifest-path tests/fixtures/native-image/Cargo.toml -- --nocapture` | 64 passed / 0 failed / 0 ignored | `C:\CodexLocal\image-metadata-native.log` |
| `cargo test --bin wx image_metadata -- --nocapture`（重复边界修复后） | 13 passed / 0 failed / 0 ignored | `C:\CodexLocal\image-metadata-query-duplicates.log` |
| `cargo test --bin wx image -- --nocapture`（含新增冷缓存与重解密） | 75 passed / 0 failed / 0 ignored | `C:\CodexLocal\image-metadata-production-final.log` |
| `cargo check --bin wx` / 目标文件 `git diff --check` | 通过；编译仅有两条 ASR 未使用项警告 | `C:\CodexLocal\image-metadata-check-final.log` |

生产构建命令显式设置 `$env:LIBCLANG_PATH='C:\CodexLocal\build-tools\libclang\clang\native'`。未设置时的首次构建环境错误保留于 `C:\CodexLocal\image-metadata-check.log`，不作为 Rust 实现失败或通过证据。

新增覆盖：15 个旧 oracle 数据变体逐项比较或明确记录严格差异；另测精确时间、raw type、重复 chat 映射、坏表/视图/侧车、仅元数据大文件、全部变体歧义；7 项真实 query 测试覆盖全局页/时间闭区间/warm 重复、默认与空页跳过坏资源、跨片重复、同片重复、同时间戳 cap 之外的重复及唯一对照、零字节和多变体、真实加密 SQLite 的冷缓存生成与过期重解密。旧 oracle 的文本格式、舍入和错误提示不是新 schema 的逐字兼容承诺。

## 精确来源

| 字段 | 旧真实读取链 | 不能替代为 |
| --- | --- | --- |
| `md5` | `decode_image.py:490 get_image_md5`：`ChatName2Id.user_name -> rowid`，查询 `MessageResourceInfo.packed_info`，由 `extract_md5_from_packed_info` 扫描标记/hex | username 的 MD5、DAT 文件名推导值、当前磁盘文件内容摘要 |
| `size` | `decode_image.py:632-644 list_chat_images`：`find_dat_files` 用 `<md5>*.dat` 搜全部月份，路径排序后取第一项，调用 `os.path.getsize` | packed_info 长度、解密图片长度、缩略图以外某个优先候选的大小、未知值填 0 |

`size` 是被选中的**加密 DAT 文件**的磁盘字节数。旧 `mcp_server.py:3440 get_chat_images` 将 size/1024 按 `.0f` 显示为 KB；0 不显示，512 字节显示 0KB，1536 字节显示 2KB。没有 DAT 时可以仍有 MD5；只有没有 MD5 时加“无资源信息”。旧列表不会解密图片或读取 DAT 正文。

资源 MD5 只是资源库中提取的元数据。现有提取器也接受扫描式 fallback，并不是完整 protobuf schema 校验；不能把它说成已经核验的明文图片 MD5。

## 历史调查：实证差异

1. 旧资源关联不带消息时间：同聊天、同 local_id 的资源按时间降序取最新。历史消息可拿到后来复用 ID 的 MD5 和 DAT size。
2. 当前 `resolver.rs:45 lookup_md5_blocking` 优先精确时间，但未命中仍取最新；精确重复资源行也取一条，不拒绝歧义。独立 Rust 探针证明这两种情况，不应直接用此函数补成“精确关联”。
3. 旧 DAT 搜索是宽泛 `<md5>*.dat` 且字典序第一项，会接受 `<md5>UNRELATED.dat`，也会选早月份缩略图。当前 `find_dat_file` 使用邻月优先、标准完整名字和 full/h/t 优先级，并非旧选择规则。
4. 当前 Rust 将 MD5 转小写；旧 marker 路径保留大小写。旧 message 查询只匹配 `local_type = 3`，当前附件查询匹配低 32 位，包含高位标志图片。不能声称字节级旧行为完全一致。
5. 当前 `q_attachments` 不查资源，分页后丢弃原始高位类型和分片信息；protocol 又只投影 local_id/time。仅改 protocol 添加 md5/size 不会产生真实数据。

## 最小真实映射

以下保留最初的设计依据；实际入口和验收见上方“当前实现与验收”。

1. 保留普通附件列表的轻量默认行为。MCP 图片列表可通过内部显式选项请求资源元数据，选项不暴露账号路径、密钥或文件名；query 复用现有跨分片消息筛选、时间边界、全局排序和分页。
2. 在消息收集到最终分页之间保留 `username + source + local_id + create_time + raw_local_type`。不能从 opaque attachment_id 反推已丢失的 raw type，不能将显示名或低 32 位类型冒充完整身份。相同身份在不同分片冲突时保留歧义状态，不任取一份。
3. 将 `native_image.rs:94 resource` 的现有只读精确资源查询最小抽取为共享 helper，由图片导出和列表复用，不能复制 SQL/校验。继续采用真实 rowid 表验证、无 shadow rowid、绑定参数、INTEGER 身份、BLOB 类型和 1 MiB 上限、精确 chat/id/raw-type/time、唯一行检查；绝不接回 latest fallback。资源库每页打开一次，在同一只读事务中查询分页身份。
4. `md5` 仅来自上述实际匹配资源行的 packed_info。资源缺失、无法提取或匹配歧义应区分；坏 schema/读取错误不能伪装成正常“没有图片”。不为缺失数据生成散列字符串。
5. 只有需要补 `size` 且已取得资源 MD5 时才检查账号绑定的 attach 根。复用 `local_files::Scan/Pin` 与图片导出现有标准候选枚举的可抽取部分，不调用 decode/export 包装器，不新建输出 guard、读密钥、读文件正文或执行 decoder。
6. size 只取已固定的真实常规 DAT 文件句柄元数据；拒绝 reparse/链接越界，核验目录及文件身份/大小快照。限定标准 `<md5>.dat`、`<md5>_h.dat`、`<md5>_t.dat`，不使用旧宽泛 glob。多个候选时不能为旧 `size` 静默选择 full/h/t 或字典序：最小安全默认是 `size=null, size_status=ambiguous`；后续若要展示各变体大小，必须单独明确字段和选择规则。
7. 返回 `md5: Option<String>`、`size: Option<u64>`，附明确资源/大小状态。size 标注 `encrypted_dat_metadata`，文件绑定仅是“精确资源行 + 标准文件名”的关联证据，不是密码学认证。缺本地文件不应清空已有 md5，stat 失败不能填 0；真实零字节文件可以为 0。不回传本地路径、packed_info、密钥或附件内部句柄。
8. protocol 只投影已经取得的真实值；对缺失/歧义保留逐行状态，不能仅因为 JSON 出现了两个字段就去掉所有 partial 声明。若要求完全保留旧文本，还需要单独复用/验证旧分页头、日期、KB 舍入和提示，而不仅是 JSON 字段补齐。

这条只读链不涉及音频、后端、云服务或任何图片解码。每页上限、扫描条目/目录上限和 packed_info 上限应沿用现有受控常量，先分页再补资源，避免给所有历史图片逐条全树扫描。同一页可合并一次有界目录扫描；不要为每条图片重新扫描整个账号。

## 写入边界

按用户最终授权，沿用当前账号正常 `DbCache::get/get_with_mode`，允许既有 WAL 更新、缓存生成和全量重解密，不额外加强为“禁止 DbCache 写入”。真实冷缓存和过期缓存 fixture 已证明这条链可用，且源数据库与 DAT 字节保持不变。列表 helper 接收正常解析后的快照，只读资源与文件元数据，不创建输出、不解码发布、不做额外无关写入。此边界不是“整个调用零磁盘写入”。

纯资源表不能提供旧 size。若连文件元数据查询也不允许，只能真实补 md5，size 必须继续缺失。即使允许 stat，也不能据此保证同权限恶意进程的所有 TOCTOU 竞争或证明文件明文归属。

## Golden 与运行

`generate_oracle.py` 只从旧源码 AST 抽取工具、分页/时间 helper、ImageResolver 的四个方法和 MD5 提取函数，不导入 mcp_server。数据库/缓存路由/联系人都明确限定为临时合成数据；旧 SQL 和文件枚举函数体不替换。UTC+8 固定本机时间依赖。调用期间 audit hook 禁止打开 DAT 正文，前后核验全部输入文件 SHA256 一致，输出目录不得出现。

`oracle.json` 固定 23 案例：缺资源/未下载/零大小/KB 舍入、同 ID 复用及缺精确时间、跨聊天、类型高位、重复精确资源、多月份/前缀候选、MD5 大小写、跨分片分页、时间闭区间、空页与非法参数。记录旧两个源码的 SHA256、函数行号、实际 SQLite/文件输入和旧结果，不把人工构造的输出 md5 当作 oracle。

```powershell
python tests/fixtures/mcp-image-listing-parity/generate_oracle.py --check
cargo test --manifest-path tests/fixtures/mcp-image-listing-parity/Cargo.toml -- --nocapture
```

最初调查阶段（2026-09-07）：23 组 AST oracle 生成并重复核验通过；Rust 14 项测试通过、0 失败/忽略，含直接引用的原 resolver/AttachmentId 测试和 3 项差异探针。该独立 Rust fixture 不链接 decoder、音频、daemon 或网络服务。完整 Rust 输出：`C:\CodexLocal\mcp-image-listing-parity.log`。这组旧探针证明来源和差异；新 query 的实现验收见本页上方新增生产测试记录。
