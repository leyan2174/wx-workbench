# Native Image 独立核心

## API

生产文件：`src/attachment/native_image.rs`。

```rust
pub fn export_image(request: ImageRequest<'_>) -> anyhow::Result<ImageOutput>;
```

`ImageRequest` 必须显式提供：

- `message: &MessageIdentity`：已验证的 username、逻辑 source、local_id、create_time、完整 local_type。
- `resource_db: &Path`：本机绝对路径，已解密且无 WAL/SHM/journal 的静态资源库。
- `attach_root: &Path`：同一账号的 `msg/attach`，不自动推断。
- `output_root: &Path`：已经存在的可信目录，位于附件源树外，不与资源库目录相同。
- `key: decoder::V2KeyMaterial`：显式 AES/XOR 参数；V2 缺 AES key 报错，V1 使用 decoder 的固定 AES key 与传入 XOR。

返回消息身份、输出与 DAT 路径、资源 rowid/MD5、DAT MD5、解码后 MD5/大小、格式、decoder 名和排序后的候选证据。

## 规则

仅接受普通 rowid 表 ChatName2Id 和 MessageResourceInfo；拒绝 view、virtual、WITHOUT ROWID 及 rowid/_rowid_/oid 显式列，包括生成列。username 使用 BINARY 精确匹配且必须只有一个映射。

资源 `(chat_id, message_local_id, message_local_type, message_create_time)` 必须唯一完全匹配；不取最新记录、不跨类型高位回退、不吞 SQL 错误。数值必须以 INTEGER 存储，packed_info 必须是有界 BLOB。只复用 resolver 的 packed_info MD5 提取函数，不调用其宽松查询或文件查找。

只扫描 `<attach_root>/<md5(username)>/<month>/Img/`。原图优先于 `_h`、`_t`，文件名大小写不敏感，保留实际路径；不接受宽前缀。相同最优等级有多个文件即失败，即使字节相同。月份不根据时间缩小范围，避免静默漏掉优先级冲突。

Windows 只读句柄锁定源与祖先，拒绝 reparse/symlink、UNC/设备/ADS/相对路径。扫描结果发布前复核；临时文件 sync 后 `persist_noclobber`，失败不覆盖已有文件、目录或硬链接。

上限：DAT 64 MiB（实际读取最多上限加一个探针字节）、资源库 128 MiB、packed_info 1 MiB、扫描与复核累计 20,000 条目、1,024 个目录句柄、128 个候选。超限明确失败，不返回截断结果。

## 验证

fixture 直接引用生产源码及现有 decoder/resolver/attachment_id，不替换实现。自己的 Cargo.toml/Cargo.lock 仅用于隔离编译；没有修改根 Cargo、公共模块或 query。

```powershell
cargo test --offline --manifest-path tests/fixtures/native-image/Cargo.toml --target x86_64-pc-windows-msvc --target-dir C:/CodexLocal/build/native-image -- --nocapture
```

全部数据为临时目录中的合成 SQLite/DAT；测试包括 legacy XOR、V1/V2 AES、缺错密钥、精确元组、重复映射/记录、异常 schema、资源/候选限制、冲突候选、junction、只读锁、源与硬链接不变及无覆盖发布。虚拟表测试构造外部构建的 sqlite_schema，不声称执行了 FTS 引擎。fixture 唯一子进程是测试用 mklink /J；生产模块不运行子进程。

## 限制

宿主支持层新增 HostOutputGuard，复用核心 Scan/Pin，不复制路径安全实现。用于输出目录隔离和显式 key 文件的 4 KiB 有界只读读取，敏感缓冲区使用 Zeroizing。允许 Windows 本机 VerbatimDisk 规范路径，仍拒绝 UNC/设备路径。相关保护测试已在主程序副本的 `image` 定向回归中执行。

- 初次独立核心验收未接公共模块；后续 main 已注册图片查询入口，独立 fixture 结果仍不等同整仓集成验收。
- 当前只支持 Windows 本机静态源。路径父目录和输出目录必须可信；不承诺对抗具有管理员权限或恶意并发控制输出目录的进程，不提供断电持久性保证。
- 消息真实性、账号、资源库与附件根关联由调用者核验；此模块不能认证随意构造的 MessageIdentity。
- 资源 MD5 是现有 marker/hex 扫描器的关联证据，并非严格 protobuf 语义解析或明文完整性认证；它可能与解码后 MD5 不同。返回 binding 明确标记为 heuristic。
- 复用 decoder 的格式魔数判定，不做完整图片格式验证。wxgf 以 decoder 的 hevc 扩展原样输出，不转码；无 ffmpeg、网络、自动 provider 或真实账号/密钥测试。
