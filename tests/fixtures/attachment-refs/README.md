# 附件只读引用测试

本夹具引用 `src/application/attachment_references.rs`、真实 `message` 模块、业务内容类型以及 `adapters/wechat/media` 的内容解码、目录布局和 legacy DAT 实现；生产查询由 daemon 调用同一实现。XML 解码和缓存命名不在夹具中复制。宿主安全测试及适配器内存测试均由本夹具注册。


## 接入契约

三个公开入口：

```rust
parse_file_message(&MessageInput<'_>) -> Result<AttachmentMetadata>
parse_record_item(&MessageInput<'_>, item_index: i64) -> Result<AttachmentMetadata>
find_reference(base: &Path, &AttachmentMetadata) -> Result<Option<FileReference>>
```

调用者先在固定账号内验证唯一消息、`base_type=49`、完整逻辑 source/local_id/time，再传 username/source/local_id/create_time/body。body 是已解压原始 XML，不是 history/export 摘要；模块不会重新查询 SQL 或声称账号已认证。source 接受 `message/message_N.db` 及 Windows 分隔符，保留大小写并规范分隔符。

文件要求直接 appmsg type=6、appattach、title；record 要求 type=19 和单层内嵌 datalist。保留直接 dataitem 的0-based索引、完整 item_count，50+项不截断；文本与 metadata-only 返回元数据，不访问文件系统、不递归展开嵌套记录。

`find_reference` 只访问显式本机绝对 base 下的 `msg/file` 或 `msg/attach/<md5(username)>/*/Rec`。它不读配置、密钥、数据库，不使用 provider，不上传/下载，不写媒体、不解 DAT。None 表示没有可返回的本地引用；通过 metadata.kind 可区分文本/metadata-only 与二进制缺失。

成功的 `FileReference` 包含 path、实际 size/MD5、binding、warning、同 hash 副本数。无消息 hash 的单候选始终是 `Heuristic`，计算实际 MD5 不会把弱绑定提升成已验证来源；多候选报歧义。有消息 hash 时只接受真实内容匹配，格式错误不能当作缺失降级。相同 hash 的多份副本按路径确定性返回，并报告 equivalent_copies；MD5 是旧内容匹配协议，不是签名或账号认证。

Windows 上结果持有只读常规文件及相关目录句柄，拒绝新的写入/删除句柄，`file()` 可从开头读取。只序列化路径再丢弃对象，会释放句柄；不保证路径之后不变，也不把整个活动账号目录声称为事务快照。扫描中状态变化明确失败。

## 安全边界

- 先检查原始路径和全部祖先，再读取；不 canonicalize 后掩盖 junction。拒绝网络/设备路径、reparse、符号链接、非普通文件、ADS、设备名、尾点/尾空格、控制字符和非法组件。
- 文件名称只接受精确匹配或 `(N)` 副本，不采用旧月份快路径 `stem*`。完整有限扫描避免坏快路径候选阻断正确副本；文件名按文本精确比较，不承诺完整 Windows Unicode 大小写折叠。
- Rec 只按对应 Img/A/V/F 目录及 item_index 查找；图片为扁平文件，非图片缺标题且已知大小时才允许 size-only。无 hash 单候选仍可能属于同聊天其他卡片，警告不可移除。
- 条目预算20,000（含最终目录复核），目录/缺失目录预算1,024，候选最多128，file递归深度最多16。任何超限都报错，不返回截断结果。
- 实际 MD5 使用64KiB缓冲，整个查找累计最多500MiB。先检查句柄大小，再对读取累计计数；增长超限最多额外读取一个探针字节后失败。不把最初 stat 当作最终限制。
- 目录集合、文件身份及属性在读取后复核，已缺失的 Rec 子目录中途出现也失败；Windows 文件句柄保持只读共享。符号链接/junction 不跳过伪装成完整扫描。
- 本机路径、只读句柄及消息 hash 不证明账号来源，也不提供硬链接来源认证；base 与消息属于同一账号由调用方保证。

## 与旧行为的有意差异

保留旧元数据字段含义、文本空白折叠、记录序号、二进制类型白名单、无 hash 单候选警告；但无效整数/负大小/无效 MD5/重复字段明确失败，不静默退化为未知。原文件名先做 Windows 安全检查，不能用 collapse 去掉尾空格后继续查找。安全改动不宣称旧错误文本逐字兼容。

旧图片解密、SQL定位、多分片歧义和MCP返回适配不属于本模块。这里不复用具有“取最新”回退的 attachment resolver，也不把 export_content 的摘要反解析成附件身份。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录运行：

```powershell
cargo test --offline --manifest-path tests/fixtures/attachment-refs/Cargo.toml -- --nocapture
```

`golden.json` 保存固定的合成元数据契约；回归直接读取该文件，不执行 Python 参考源码。夹具中的路径、摘要、索引和大记录测试应保留源字节及目录集合不变断言；符号链接权限不足须单列跳过。

消息定位和 MCP 返回边界见[附件契约](../../../docs/native-attachment-contract.md)，畸形标量的拒绝断言见[查询安全回归](../native-attachment-security/README.md)。
