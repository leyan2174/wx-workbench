# MCP 图片查询适配

> 2026-09-07 文档核对：生产查询和宿主已接线，下方 raw-keys 与 host-wrapper 数字均属于各次历史快照，不是本次运行结果。当前优先从仓库根运行 `cargo test --bin wx daemon::query::mcp_image::tests -- --nocapture`；本次未执行。保留 `run.ps1`、测试源码和旧日志，新复跑不要覆盖历史证据。

## API

宿主应调用以下入口，低层 q_decode_image 仍供已准备好密钥材料的内部调用者使用：

```rust
pub async fn q_decode_image_with_key_file(
    db: &DbCache, names: &Names, chat: &str,
    local_id: i64, create_time: i64,
    output_root: &Path, key_file: Option<&Path>,
) -> anyhow::Result<serde_json::Value>;
```

```rust
pub async fn q_decode_image(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
    key: V2KeyMaterial<'_>,
) -> anyhow::Result<serde_json::Value>;
```

先调用真实 `strict_message::locate`，唯一性判断先于图片类型过滤。`create_time=0` 只表示消息定位时不筛时间，导出时仍传命中消息的真实时间。`kind` 保留全部高位，只有 base_type=3 才进入图片核心。

成功返回 `{"exit_code":0,"status":"published","image":ImageOutput}`，ImageOutput 包含完整消息身份、实际输出路径、源 DAT 路径和校验证据。聊天或消息不存在返回 exit_code=1；聊天或消息身份歧义返回 exit_code=2。清单、缓存、资源 schema、解码或发布错误通过 Result 传播。

## 宿主依赖

- main 负责真实模块、IPC/MCP 注册和 CLI 宿主参数注入。fixture 只使用真实注册，不补写模块或接口。
- 宿主传入固定账号的 DbCache/Names；附件根唯一来源是 `db.db_dir().parent()/msg/attach`。
- 宿主必须显式提供 output_root 和 V2KeyMaterial；适配器不读取配置，不使用旧 q_extract，不调用自动 provider、网络或 ffmpeg。阻塞任务持有的 AES 副本在结束时清零。
- 使用现有 DbCache 只读 `raw_db_keys()` 全键接口：适配器规范化斜线和大小写精确筛选 `message/message_resource.db`，保留原始键、不去重。拒绝零个或多个结果，再以原始键调用 db.get，不新增资源专用接口。
- `DbCache::output_protection_paths()` 返回 source、缓存根、mtime、已有解密产物，以及初始化上下文中的 config/keys/decrypted 路径。缓存元数据锁占用时拒绝返回部分清单；查询时不重新加载配置。内部 with_dirs 构造的合成缓存没有外部配置文件，真实 new 构造保留初始化上下文路径。

运行器原样复制真实 cache.rs 并核对 SHA256。缺少 raw_db_keys 时立即停止，绝不注入替代接口。

后续获明确授权后，本任务在 cache.rs 中补入上述输出保护路径 API，在 toolkit/mod.rs 中加入两个薄包装 `parse_image_aes/parse_image_xor`，调用原有 CLI 解析函数；未修改其解析语义，也未增加配置查找或 provider。

## 显式 Key 文件

宿主输出目录必须已存在且为绝对本机路径；空值、相对路径、UNC/设备路径先于账号和 key 文件访问失败。支持 Windows 本机规范盘符路径。source/cache 等目录不能与输出目录双向重叠；config、keys、mtime 和 image key 等保护文件不能位于输出目录内。目录守卫持有到导出结束。

image key 文件必须为显式绝对路径的常规文件，拒绝 reparse/symlink 和目录，最多 4 KiB，读取最多上限加一个探针字节；只读句柄拒绝并发写入/替换。文件缓冲区、已解析的字符串和密钥材料使用零化包装；错误不携带 key 值、key 文件名或原始错误链。

```json
{"aes_key":"1234567890abcdef","xor_key":"0xa2"}
```

字段为可选的 aes_key 字符串和 xor_key 整数或字符串，拒绝重复字段与未知字段。AES 完全沿用现有 CLI 规则：至少 16 个 ASCII 字节，取前 16 字节，不把字符串自动视为十六进制密钥。XOR 支持 0..255 整数、十进制字符串或 0x/0X 十六进制字符串，默认 0x88。没有 key 文件时仍允许 legacy/V1；V2 缺少 AES key 会失败，不尝试自动获取。

## 写入边界

加载资源缓存前捕获当前账号的消息与资源源文件清单；限定只使用 message/message_resource.db，拒绝额外 message_*resource*.db、源缺失、目录冒充、重解析点和未知消息分片。缓存加载后，在最终阻塞任务中重做完整性检查，并比较源文件身份、大小、修改时间及 WAL 的存在/属性。

所有清单复核都发生在 native_image::export_image 之前。导出核心负责有界读取、静态缓存资源检查、附件路径固定以及无覆盖发布。成功发布后只构造响应，不再做路径打开或清单检查，以免已生成图片却因可预见后置错误丢失成功记录。

清单捕获是观察到的账号快照，不是整个微信目录的全局事务锁，不承诺对抗特权进程伪造文件时间。宿主必须持续等待已派发的阻塞任务并保存成功响应；进程崩溃或取消 await 无法提供持久响应日志保证。本适配器不另建持久输出索引。

native_image 的 Windows、静态资源库、可信输出根、MD5 弱绑定及 wxgf 不转码等限制仍然适用。

## 验证

`run.ps1` 在 C:/CodexLocal/build 下建立唯一临时源码副本，不改写源代码或公共注册。测试使用真实 DbCache 的持久缓存命中、真实 strict_message、真实 native_image 和 decoder；没有伪造缓存或查询实现，没有真实账号。

```powershell
& tests/fixtures/mcp-image/run.ps1 -TargetDir C:/CodexLocal/src/wx-cli/target
```

运行日志写在本 fixture 中；临时副本路径会打印并保留供复核。只运行 mcp_image 测试过滤器，不声称执行整仓测试。原始 keys 大小写/反斜线、重复别名、未知/缺失资源和消息、类型/时间、歧义、资源损坏、显式 V2 密钥、清单变化和无覆盖发布均由合成数据验证。

旧 `run.log` / `tests.log` 是此前补入资源专用接口的 12 项结果，仅保留历史，不作为后续 raw_db_keys 验收。`raw-keys-tests.log` 和 `raw-keys-check.log` 保存真实缓存接口阶段的结果。当前运行器实际使用固定的 `host-wrapper-tests.log` / `host-wrapper-check.log`，直接复跑会覆盖它们；保留旧证据时使用页首主仓入口并另存新输出。

旧 `check-run.log` / `check-retry.log` 同样只保留历史。初次独立 check 缺 LIBCLANG_PATH 的失败日志保留在 `check.log`，运行器现已统一环境。使用 `run.ps1 -Check` 可复跑非测试构建；实际图片 MCP 入口及 output/key 注入已由 main 注册，不属于这些旧日志的验收范围。

真实 raw_db_keys 验收：13 项通过，0 失败、0 忽略，539 项其他测试被过滤；非测试 check 退出码 0，有 9 条尚未接入图片调用链的未使用代码警告。见 `raw-keys-run.log` / `raw-keys-tests.log` / `raw-keys-check-run.log` / `raw-keys-check.log`。两轮均校验原样复制的 cache.rs SHA256 为 `039EE9504F4BC81A9A74401CB110BBCBFACD28DC873740DB542FE81DD97867BE`，未注入任何缓存接口。

历史宿主 wrapper 验收：`run.ps1 -TestFilter image` 实跑 59 项通过、0 失败、0 忽略，514 项被过滤，无编译警告；包含 19 项 mcp_image 测试、新缓存保护 API 测试、原生保护和 main 的图片协议/宿主相关测试。见 `host-wrapper-verified-run.log` / `host-wrapper-tests.log`。该轮 cache.rs 原样复制 SHA256 为 `F71CF1F965824FF070C748CAE450A8988DC515597DE4064A827DC4F618DCA816`。此前 `host-wrapper-run.log` 的重复模块注册错误来自旧运行器，现已删除所有副本注册逻辑，不作为该轮最终结果。
